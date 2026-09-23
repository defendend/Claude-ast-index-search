#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// Every test gets its own project root, its own DB and its own git repo;
/// nothing here may touch the developer's real cache.
struct Workspace {
    _temp: TempDir,
    root: PathBuf,
    db: PathBuf,
    cache: PathBuf,
}

fn workspace() -> Workspace {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    let db = temp.path().join("index.db");
    let cache = temp.path().join("cache");
    Workspace {
        _temp: temp,
        root,
        db,
        cache,
    }
}

impl Workspace {
    fn ast_index(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&self.root)
            .env("AST_INDEX_DB_PATH", &self.db)
            .env("AST_INDEX_CACHE_DIR", &self.cache)
            .env_remove("AST_INDEX_VCS_BIN")
            .args(args)
            .output()
            .expect("ast-index must run")
    }

    fn ast_index_with_vcs(&self, fake_vcs: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&self.root)
            .env("AST_INDEX_DB_PATH", &self.db)
            .env("AST_INDEX_CACHE_DIR", &self.cache)
            .env("AST_INDEX_VCS_BIN", fake_vcs)
            .args(args)
            .output()
            .expect("ast-index must run")
    }

    fn rebuild(&self) {
        assert_success(&self.ast_index(&["rebuild"]));
    }

    fn hotspots_json(&self, args: &[&str]) -> Value {
        let mut all = vec!["hotspots", "--format", "json"];
        all.extend_from_slice(args);
        let output = self.ast_index(&all);
        assert_success(&output);
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "hotspots must emit JSON: {error}; stdout={}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn git(&self, args: &[&str]) {
        git_in(&self.root, args);
    }

    fn commit(&self, message: &str, files: &[(&str, &str)]) {
        for (path, contents) in files {
            let target = self.root.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&target, contents).unwrap();
            self.git(&["add", path]);
        }
        self.git(&["commit", "-m", message]);
    }

    fn init_git(&self) {
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.email", "dev@example.invalid"]);
        self.git(&["config", "user.name", "Dev One"]);
    }

    /// Commit with both dates pinned, so a test can put a child before its
    /// parent on the clock.
    fn commit_at(&self, message: &str, files: &[(&str, &str)], at: i64) {
        for (path, contents) in files {
            let target = self.root.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&target, contents).unwrap();
            self.git(&["add", path]);
        }
        self.git_at(&["commit", "-q", "-m", message], at);
    }

    fn git_at(&self, args: &[&str], at: i64) {
        let date = format!("@{at} +0000");
        let output = git_command(&self.root, args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .expect("git must run");
        assert_git_success(args, &output);
    }

    /// Collect incrementally and check the tables against a fresh full
    /// collection at the same HEAD.
    fn collect_and_compare(&self, label: &str) -> Value {
        let json = self.hotspots_json(&["--collect"]);
        assert_eq!(
            json["collection"]["mode"], "incremental",
            "{label}: {}",
            json["collection"]
        );
        self.assert_matches_fresh_collection(label);
        json
    }

    /// The collected tables must equal, row for row and column for column,
    /// what a from-scratch collection at the current HEAD writes.
    fn assert_matches_fresh_collection(&self, label: &str) {
        let fresh = self.db.with_file_name("fresh.db");
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", fresh.display()));
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute("VACUUM INTO ?1", [fresh.to_str().unwrap()])
            .unwrap();
        drop(conn);

        let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&self.root)
            .env("AST_INDEX_DB_PATH", &fresh)
            .env("AST_INDEX_CACHE_DIR", &self.cache)
            .env_remove("AST_INDEX_VCS_BIN")
            .args(["hotspots", "--collect", "--full", "--format", "json"])
            .output()
            .expect("ast-index must run");
        assert_success(&output);

        let incremental = CollectedSignals::read(&self.db);
        let from_scratch = CollectedSignals::read(&fresh);
        assert!(
            !from_scratch.stats.is_empty() || label.contains("empty"),
            "{label}: the fresh collection found nothing to compare"
        );
        assert_eq!(incremental, from_scratch, "{label}");
    }
}

/// Everything a collection derives, as raw SQLite values.
#[derive(Debug, PartialEq)]
struct CollectedSignals {
    stats: Vec<Vec<rusqlite::types::Value>>,
    authors: Vec<Vec<rusqlite::types::Value>>,
    metadata: Vec<Vec<rusqlite::types::Value>>,
}

impl CollectedSignals {
    fn read(db: &Path) -> CollectedSignals {
        let conn = rusqlite::Connection::open(db).unwrap();
        let dump = |sql: &str| -> Vec<Vec<rusqlite::types::Value>> {
            let mut statement = conn.prepare(sql).unwrap();
            let columns = statement.column_count();
            statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| row.get::<_, rusqlite::types::Value>(index))
                        .collect()
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        CollectedSignals {
            stats: dump("SELECT * FROM git_file_stats ORDER BY path"),
            authors: dump("SELECT * FROM git_file_authors ORDER BY path, author"),
            metadata: dump(
                "SELECT key, value FROM metadata
                 WHERE key IN ('git_signals_commits', 'git_signals_paths', 'git_signals_head')
                 ORDER BY key",
            ),
        }
    }
}

fn git_command(repo: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Dev One")
        .env("GIT_AUTHOR_EMAIL", "dev@example.invalid")
        .env("GIT_COMMITTER_NAME", "Dev One")
        .env("GIT_COMMITTER_EMAIL", "dev@example.invalid")
        .args(args);
    command
}

fn assert_git_success(args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "git {:?}: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_in(repo: &Path, args: &[&str]) {
    let output = git_command(repo, args).output().expect("git must run");
    assert_git_success(args, &output);
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_fake_vcs(root: &Path, body: &str) -> PathBuf {
    let script = root.join("fake-vcs");
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    script
}

fn find_item<'a>(json: &'a Value, path: &str) -> &'a Value {
    json["items"]
        .as_array()
        .expect("items must be an array")
        .iter()
        .find(|item| item["path"] == path)
        .unwrap_or_else(|| panic!("no hotspot row for {path}; got {}", json["items"]))
}

#[test]
fn hotspots_reports_nothing_before_collection() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add module", &[("src/a.rs", "fn a() {}\n")]);
    workspace.rebuild();

    let output = workspace.ast_index(&["hotspots"]);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No git signals collected yet"), "{stdout}");

    // A plain rebuild must not have populated the tables behind the user's back.
    let json = workspace.hotspots_json(&[]);
    assert_eq!(json["files_with_history"], 0);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);
}

#[test]
fn collect_accumulates_commits_churn_fixes_and_authors() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/parser.rs", "fn a() {}\n")]);
    workspace.commit(
        "Fix crash in parser",
        &[("src/parser.rs", "fn a() {}\nfn b() {}\n")],
    );
    workspace.commit(
        "[PTK-42] Исправить падение парсера",
        &[("src/parser.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
    );
    workspace.commit("Add docs", &[("README.md", "docs\n")]);
    workspace.rebuild();

    let json = workspace.hotspots_json(&["--collect"]);
    assert_eq!(json["collection"]["mode"], "full");
    assert_eq!(json["collection"]["commits_scanned"], 4);
    assert_eq!(json["files_with_history"], 2);

    let parser = find_item(&json, "src/parser.rs");
    assert_eq!(parser["commits"], 3);
    assert_eq!(parser["fix_commits"], 2);
    assert_eq!(parser["lines_added"], 3);
    assert_eq!(parser["lines_deleted"], 0);
    assert_eq!(parser["churn"], 3);
    assert_eq!(parser["authors"], 1);
    assert_eq!(parser["current_lines"], 3);
    assert!(parser["first_commit_at"].as_i64().unwrap() > 0);

    let readme = find_item(&json, "README.md");
    assert_eq!(readme["commits"], 1);
    assert_eq!(readme["fix_commits"], 0);
}

#[test]
fn second_collect_only_reads_new_commits() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/parser.rs", "fn a() {}\n")]);
    workspace.commit("Add lexer", &[("src/lexer.rs", "fn l() {}\n")]);
    workspace.rebuild();

    let first = workspace.hotspots_json(&["--collect"]);
    assert_eq!(first["collection"]["mode"], "full");
    assert_eq!(first["collection"]["commits_scanned"], 2);
    let head_after_first = first["head"].as_str().unwrap().to_string();

    // Nothing new: the cursor equals HEAD, so zero commits are re-read.
    let idle = workspace.hotspots_json(&["--collect"]);
    assert_eq!(idle["collection"]["mode"], "incremental");
    assert_eq!(idle["collection"]["commits_scanned"], 0);
    assert_eq!(idle["collection"]["paths_touched"], 0);
    assert_eq!(idle["head"], head_after_first.as_str());
    assert_eq!(find_item(&idle, "src/parser.rs")["commits"], 1);

    workspace.commit(
        "Fix lexer regression",
        &[("src/lexer.rs", "fn l() {}\nfn m() {}\n")],
    );
    let incremental = workspace.hotspots_json(&["--collect"]);
    assert_eq!(incremental["collection"]["mode"], "incremental");
    assert_eq!(incremental["collection"]["commits_scanned"], 1);
    assert_eq!(incremental["collection"]["paths_touched"], 1);
    assert_eq!(
        incremental["collection"]["previous_head"],
        head_after_first.as_str()
    );

    let lexer = find_item(&incremental, "src/lexer.rs");
    assert_eq!(lexer["commits"], 2);
    assert_eq!(lexer["fix_commits"], 1);
    // Untouched file keeps its earlier totals rather than being recounted.
    assert_eq!(find_item(&incremental, "src/parser.rs")["commits"], 1);
}

#[test]
fn full_flag_recollects_from_scratch_without_double_counting() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/parser.rs", "fn a() {}\n")]);
    workspace.commit("Fix parser", &[("src/parser.rs", "fn a() {}\nfn b() {}\n")]);
    workspace.rebuild();

    let first = workspace.hotspots_json(&["--collect"]);
    assert_eq!(find_item(&first, "src/parser.rs")["commits"], 2);

    let refreshed = workspace.hotspots_json(&["--collect", "--full"]);
    assert_eq!(refreshed["collection"]["mode"], "full");
    assert_eq!(refreshed["collection"]["commits_scanned"], 2);
    assert_eq!(find_item(&refreshed, "src/parser.rs")["commits"], 2);
}

#[test]
fn rewritten_history_is_subtracted_without_a_full_recollect() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/parser.rs", "fn a() {}\n")]);
    workspace.commit("Add throwaway", &[("src/throwaway.rs", "fn t() {}\n")]);
    workspace.rebuild();

    let first = workspace.hotspots_json(&["--collect"]);
    assert_eq!(first["files_with_history"], 2);
    let discarded_head = first["head"].as_str().unwrap().to_string();

    // Drop the tip commit: the stored cursor is no longer an ancestor of HEAD,
    // which is what a rebase, a force-push or a branch switch looks like.
    workspace.git(&["reset", "--hard", "HEAD~1"]);

    let moved = workspace.hotspots_json(&["--collect"]);
    assert_eq!(moved["collection"]["mode"], "incremental");
    assert_eq!(moved["collection"]["commits_scanned"], 0);
    assert_eq!(moved["collection"]["commits_dropped"], 1);
    assert!(moved["collection"].get("reset_reason").is_none());
    assert_ne!(moved["head"], discarded_head.as_str());
    assert_eq!(moved["files_with_history"], 1);
    assert_eq!(moved["commits_analyzed"], 1);
    assert_eq!(moved["items"].as_array().unwrap().len(), 1);

    // Back to the discarded tip: its diff is still in the store.
    workspace.git(&["reset", "--hard", &discarded_head]);
    let restored = workspace.hotspots_json(&["--collect"]);
    assert_eq!(restored["collection"]["mode"], "incremental");
    assert_eq!(restored["collection"]["commits_scanned"], 0);
    assert_eq!(restored["collection"]["commits_restored"], 1);
    assert_eq!(restored["files_with_history"], 2);
    workspace.assert_matches_fresh_collection("back at the discarded tip");
}

#[test]
fn unknown_stored_cursor_does_not_abort_collection() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/parser.rs", "fn a() {}\n")]);
    workspace.rebuild();
    assert_success(&workspace.ast_index(&["hotspots", "--collect"]));

    // A commit id that does not exist in this repository at all, as if the
    // objects it named had been garbage-collected away.
    let bogus = "0".repeat(40);
    let conn = rusqlite::Connection::open(&workspace.db).unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('git_signals_head', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [&bogus],
    )
    .unwrap();
    drop(conn);

    let json = workspace.hotspots_json(&["--collect"]);
    assert_eq!(json["collection"]["mode"], "full");
    assert_eq!(json["collection"]["commits_scanned"], 1);
    assert_eq!(find_item(&json, "src/parser.rs")["commits"], 1);
}

#[test]
fn collect_outside_a_git_repository_fails_with_a_clear_message() {
    let workspace = workspace();
    fs::write(workspace.root.join("a.rs"), "fn a() {}\n").unwrap();
    workspace.rebuild();

    let output = workspace.ast_index(&["hotspots", "--collect"]);
    assert!(
        !output.status.success(),
        "collection must not silently pass"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no Git or Arc working tree found"),
        "stderr={stderr}"
    );

    // Reporting without --collect stays usable and simply has nothing to show.
    let output = workspace.ast_index(&["hotspots"]);
    assert_success(&output);
}

#[test]
fn collect_rejects_a_non_git_working_tree() {
    let workspace = workspace();
    fs::create_dir_all(workspace.root.join(".arc")).unwrap();
    fs::write(workspace.root.join(".arc").join("HEAD"), "ref: trunk\n").unwrap();
    fs::write(workspace.root.join("a.rs"), "fn a() {}\n").unwrap();
    workspace.rebuild();

    let fake = write_fake_vcs(workspace.root.parent().unwrap(), "printf ''");
    let output = workspace.ast_index_with_vcs(&fake, &["hotspots", "--collect"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git signals need a Git working tree"),
        "{stderr}"
    );
}

#[test]
fn a_path_recreated_after_a_rename_starts_from_zero() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/old.rs", "fn a() {}\n")]);
    workspace.commit("Fix parser", &[("src/old.rs", "fn a() {}\nfn b() {}\n")]);
    workspace.rebuild();

    // Collect first, so the pre-rename history for src/old.rs is on disk when
    // the incremental run sees both the rename and the new file.
    let first = workspace.hotspots_json(&["--collect"]);
    assert_eq!(find_item(&first, "src/old.rs")["commits"], 2);

    workspace.git(&["mv", "src/old.rs", "src/new.rs"]);
    workspace.git(&["commit", "-m", "Move parser to its own module"]);
    workspace.commit(
        "Add an unrelated module at the freed path",
        &[("src/old.rs", "fn unrelated() {}\n")],
    );

    let second = workspace.hotspots_json(&["--collect"]);
    assert_eq!(second["collection"]["mode"], "incremental");
    // The moved file keeps everything; the new tenant of the old path does
    // not inherit a history it never had.
    assert_eq!(find_item(&second, "src/new.rs")["commits"], 3);
    assert_eq!(find_item(&second, "src/new.rs")["fix_commits"], 1);
    assert_eq!(find_item(&second, "src/old.rs")["commits"], 1);
    assert_eq!(find_item(&second, "src/old.rs")["fix_commits"], 0);
}

#[test]
fn renames_carry_history_onto_the_new_path() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/old.rs", "fn a() {}\n")]);
    workspace.commit("Fix parser", &[("src/old.rs", "fn a() {}\nfn b() {}\n")]);
    workspace.git(&["mv", "src/old.rs", "src/new.rs"]);
    workspace.git(&["commit", "-m", "Move parser to its own module"]);
    workspace.rebuild();

    let json = workspace.hotspots_json(&["--collect"]);
    let renamed = find_item(&json, "src/new.rs");
    assert_eq!(renamed["commits"], 3);
    assert_eq!(renamed["fix_commits"], 1);
    assert!(
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["path"] != "src/old.rs"),
        "the old path must not linger: {}",
        json["items"]
    );
}

#[test]
fn labels_and_percentiles_are_relative_to_this_repository() {
    let workspace = workspace();
    workspace.init_git();
    // One file moves constantly, nine are written once and left alone.
    for index in 0..9 {
        workspace.commit(
            &format!("Add module {index}"),
            &[(&format!("src/quiet{index}.rs"), "fn quiet() {}\n")],
        );
    }
    let mut body = String::new();
    for index in 0..30 {
        body.push_str(&format!("fn hot{index}() {{}}\n"));
        let message = if index % 2 == 0 {
            format!("Fix hot path {index}")
        } else {
            format!("Extend hot path {index}")
        };
        workspace.commit(&message, &[("src/hot.rs", &body)]);
    }
    workspace.rebuild();

    let json = workspace.hotspots_json(&["--collect", "--limit", "50"]);
    let hot = find_item(&json, "src/hot.rs");
    assert_eq!(hot["commits"], 30);
    assert_eq!(hot["fix_commits"], 15);
    assert!(hot["churn_pct"].as_u64().unwrap() >= 90);
    assert!(hot["commits_pct"].as_u64().unwrap() >= 90);
    let labels: Vec<&str> = hot["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| label.as_str().unwrap())
        .collect();
    assert!(labels.contains(&"churn:high"), "{labels:?}");
    assert!(labels.contains(&"fixes:high"), "{labels:?}");

    let quiet = find_item(&json, "src/quiet0.rs");
    assert_eq!(quiet["commits"], 1);
    assert!(quiet["churn_pct"].as_u64().unwrap() < 90);
    // A one-commit file must never earn a fix label from a 0/1 or 1/1 ratio.
    assert!(
        !quiet["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label.as_str().unwrap().starts_with("fixes:")),
        "{}",
        quiet["labels"]
    );
    // The same numbers in a repository where every file is hot would rank
    // differently: percentiles are computed from this population only.
    assert_eq!(json["files_with_history"], 10);
}

#[test]
fn filters_sorting_and_pagination_apply_after_percentiles() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add app entry", &[("app/main.rs", "fn main() {}\n")]);
    workspace.commit("Add lib entry", &[("lib/util.rs", "fn util() {}\n")]);
    workspace.commit(
        "Fix app entry",
        &[("app/main.rs", "fn main() {}\nfn extra() {}\n")],
    );
    workspace.rebuild();
    assert_success(&workspace.ast_index(&["hotspots", "--collect"]));

    let filtered = workspace.hotspots_json(&["--path", "app/"]);
    let paths: Vec<&str> = filtered["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["app/main.rs"]);
    // Percentiles come from all 2 files, not just the filtered one.
    assert_eq!(filtered["files_with_history"], 2);

    let min_commits = workspace.hotspots_json(&["--min-commits", "2"]);
    assert_eq!(min_commits["items"].as_array().unwrap().len(), 1);

    let paged = workspace.hotspots_json(&["--limit", "1"]);
    assert_eq!(paged["pagination"]["total"], 2);
    assert_eq!(paged["pagination"]["returned"], 1);
    assert_eq!(paged["pagination"]["truncated"], true);
    assert_eq!(paged["schema_version"], 2);

    let sorted = workspace.hotspots_json(&["--sort", "commits"]);
    assert_eq!(sorted["items"][0]["path"], "app/main.rs");

    let bad_sort = workspace.ast_index(&["hotspots", "--sort", "nonsense"]);
    assert!(!bad_sort.status.success());
    assert!(String::from_utf8_lossy(&bad_sort.stderr).contains("--sort must be one of"));
}

#[test]
fn hotspots_rejects_subtree_scoping() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/parser.rs", "fn a() {}\n")]);
    workspace.rebuild();

    let output = workspace.ast_index(&["--subtree", "whatever", "hotspots"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--subtree is not supported by 'hotspots'"));
}

#[test]
fn collection_survives_a_window_smaller_than_the_history() {
    let workspace = workspace();
    workspace.init_git();
    for index in 0..7 {
        workspace.commit(
            &format!("Add step {index}"),
            &[("src/step.rs", &format!("fn step{index}() {{}}\n"))],
        );
    }
    workspace.git(&["mv", "src/step.rs", "src/renamed.rs"]);
    workspace.git(&["commit", "-m", "Rename the step module"]);
    workspace.commit(
        "Fix the renamed module",
        &[("src/renamed.rs", "fn step6() {}\nfn extra() {}\n")],
    );
    workspace.rebuild();

    // Window 2 forces several separate `git log --skip/-n` slices; the totals
    // must match a single-slice run exactly, and a rename that lands in one
    // slice must still absorb history accumulated in an older slice.
    let windowed = workspace.hotspots_json(&["--collect", "--window", "2"]);
    assert_eq!(windowed["collection"]["commits_scanned"], 9);
    assert_eq!(find_item(&windowed, "src/renamed.rs")["commits"], 9);

    let single = workspace.hotspots_json(&["--collect", "--full", "--window", "1000"]);
    for field in [
        "commits",
        "fix_commits",
        "churn",
        "lines_added",
        "lines_deleted",
    ] {
        assert_eq!(
            find_item(&single, "src/renamed.rs")[field],
            find_item(&windowed, "src/renamed.rs")[field],
            "{field} must not depend on the window size"
        );
    }
    assert_eq!(single["paths_in_history"], windowed["paths_in_history"]);
}

#[test]
fn deleted_files_count_in_history_but_drop_out_of_the_report() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add temp", &[("src/temp.rs", "fn t() {}\n")]);
    workspace.commit("Add kept", &[("src/kept.rs", "fn k() {}\n")]);
    workspace.git(&["rm", "src/temp.rs"]);
    workspace.git(&["commit", "-m", "Drop the temporary module"]);
    workspace.rebuild();

    let json = workspace.hotspots_json(&["--collect"]);
    let paths: Vec<&str> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["src/kept.rs"]);
    // The deleted path still counts as history, but it is not part of the
    // population percentiles are computed against.
    assert_eq!(json["paths_in_history"], 2);
    assert_eq!(json["files_with_history"], 1);

    // Resurrecting it carries its earlier commits: the per-commit store keeps
    // them even though the derived table has no row for a deleted path.
    workspace.commit("Bring the module back", &[("src/temp.rs", "fn t() {}\n")]);
    let json = workspace.hotspots_json(&["--collect"]);
    assert_eq!(find_item(&json, "src/temp.rs")["commits"], 3);
    assert_eq!(json["paths_in_history"], 2);
    workspace.assert_matches_fresh_collection("after the resurrection");
}

#[test]
fn history_outside_the_project_root_is_ignored() {
    let workspace = workspace();
    let repo = workspace.root.parent().unwrap().to_path_buf();
    git_in(&repo, &["init", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "dev@example.invalid"]);
    git_in(&repo, &["config", "user.name", "Dev One"]);
    fs::write(repo.join("outside.rs"), "fn outside() {}\n").unwrap();
    fs::write(workspace.root.join("inside.rs"), "fn inside() {}\n").unwrap();
    git_in(&repo, &["add", "outside.rs", "project/inside.rs"]);
    git_in(&repo, &["commit", "-m", "Add both"]);
    workspace.rebuild();

    let json = workspace.hotspots_json(&["--collect"]);
    let paths: Vec<&str> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["inside.rs"]);
}

fn at(hour: i64) -> i64 {
    1_700_000_000 + hour * 3600
}

#[test]
fn branch_switches_rebases_and_merges_match_a_fresh_full_collection() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit_at(
        "Add parser, lexer and widget",
        &[
            ("src/a.rs", "a\n"),
            ("src/b.rs", "b\n"),
            ("src/w.rs", "w\n"),
        ],
        at(1),
    );
    workspace.commit_at("Fix parser crash", &[("src/a.rs", "a\na2\n")], at(2));
    workspace.git(&["branch", "side"]);
    workspace.git(&["mv", "src/w.rs", "src/v.rs"]);
    workspace.git_at(&["commit", "-q", "-m", "Move widget"], at(3));
    workspace.commit_at("Add config", &[("src/c.rs", "c\n")], at(4));
    workspace.rebuild();
    let first = workspace.hotspots_json(&["--collect"]);
    assert_eq!(first["collection"]["mode"], "full");
    workspace.assert_matches_fresh_collection("main");

    // A sibling that forked before main renamed the widget: it keeps editing
    // the old path, renames the parser and reuses the parser's freed name.
    workspace.git(&["checkout", "-q", "side"]);
    workspace.commit_at("Extend lexer", &[("src/b.rs", "b\nb2\n")], at(5));
    workspace.git(&["mv", "src/a.rs", "src/d.rs"]);
    workspace.git_at(&["commit", "-q", "-m", "Move parser"], at(6));
    workspace.commit_at("Fix moved parser", &[("src/d.rs", "a\na2\nd3\n")], at(7));
    workspace.commit_at("Tweak widget", &[("src/w.rs", "w\nw2\n")], at(8));
    let side = workspace.collect_and_compare("switched to the sibling branch");
    assert_eq!(side["collection"]["commits_dropped"], 2);
    assert_eq!(side["collection"]["commits_scanned"], 4);
    assert_eq!(find_item(&side, "src/d.rs")["commits"], 4);

    workspace.git(&["checkout", "-q", "main"]);
    let back = workspace.collect_and_compare("back on main");
    assert_eq!(back["collection"]["commits_scanned"], 0);
    assert_eq!(back["collection"]["commits_restored"], 2);
    assert_eq!(back["collection"]["commits_dropped"], 4);
    assert_eq!(find_item(&back, "src/a.rs")["commits"], 2);

    // Merge the sibling in, with a merge commit dated before both parents.
    workspace.git_at(&["merge", "-q", "--no-edit", "side"], at(0));
    let merged = workspace.collect_and_compare("after merging the sibling");
    assert_eq!(merged["collection"]["commits_restored"], 4);
    assert_eq!(merged["collection"]["commits_scanned"], 0);

    // A child committed before its parent on the clock.
    workspace.commit_at("Fix config", &[("src/c.rs", "c\nc2\n")], at(1));
    workspace.collect_and_compare("child older than its parent");

    // A topic branch that renames its own file, rebased onto a moved main.
    workspace.git(&["checkout", "-q", "-b", "topic"]);
    workspace.commit_at("Add feature flag", &[("src/e.rs", "e\n")], at(10));
    workspace.git(&["mv", "src/e.rs", "src/f.rs"]);
    workspace.git_at(&["commit", "-q", "-m", "Rename feature flag"], at(11));
    workspace.collect_and_compare("topic branch");
    workspace.git(&["checkout", "-q", "main"]);
    workspace.commit_at(
        "Fix lexer regression",
        &[("src/b.rs", "b\nb2\nb3\n")],
        at(12),
    );
    workspace.git(&["checkout", "-q", "topic"]);
    workspace.git_at(&["rebase", "-q", "main"], at(13));
    let rebased = workspace.collect_and_compare("topic rebased onto main");
    assert_eq!(rebased["collection"]["commits_dropped"], 2);
    assert_eq!(rebased["collection"]["commits_scanned"], 3);

    workspace.git(&["reset", "-q", "--hard", "HEAD~3"]);
    workspace.collect_and_compare("hard reset three commits back");

    // An unrelated history shares no commit with the stored one.
    workspace.git(&["checkout", "-q", "--orphan", "lonely"]);
    workspace.git(&["rm", "-r", "-q", "-f", "."]);
    workspace.commit_at("Start over", &[("src/a.rs", "unrelated\n")], at(20));
    workspace.collect_and_compare("orphan branch");

    workspace.git(&["checkout", "-q", "main"]);
    workspace.collect_and_compare("back on main after the orphan");
}

#[test]
fn a_project_below_the_repository_root_survives_branch_switches() {
    let workspace = workspace();
    let repo = workspace.root.parent().unwrap().to_path_buf();
    git_in(&repo, &["init", "-b", "main"]);
    let commit = |message: &str, files: &[(&str, &str)], hour: i64| {
        for (path, contents) in files {
            let target = repo.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, contents).unwrap();
            git_in(&repo, &["add", path]);
        }
        let date = format!("@{} +0000", at(hour));
        let output = git_command(&repo, &["commit", "-q", "-m", message])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap();
        assert_git_success(&["commit"], &output);
    };
    commit(
        "Add both sides",
        &[
            ("project/src/in.rs", "in\n"),
            ("shared/out.rs", "out\n"),
            ("project/src/stay.rs", "stay\n"),
        ],
        1,
    );
    git_in(&repo, &["branch", "side"]);
    commit("Fix inside", &[("project/src/in.rs", "in\nfix\n")], 2);
    workspace.rebuild();
    workspace.hotspots_json(&["--collect"]);
    workspace.assert_matches_fresh_collection("main");

    // Files cross the project boundary on the side branch in both directions.
    git_in(&repo, &["checkout", "-q", "side"]);
    git_in(&repo, &["mv", "shared/out.rs", "project/src/moved_in.rs"]);
    git_in(&repo, &["mv", "project/src/stay.rs", "shared/stay.rs"]);
    commit("Move files across the boundary", &[], 3);
    commit(
        "Fix moved file",
        &[("project/src/moved_in.rs", "out\nfix\n")],
        4,
    );
    workspace.collect_and_compare("side branch below the repository root");
    git_in(&repo, &["checkout", "-q", "main"]);
    workspace.collect_and_compare("main again below the repository root");
}

#[test]
fn pruning_the_store_keeps_results_exact() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit_at("Add parser", &[("src/a.rs", "a\n")], at(1));
    workspace.git(&["branch", "side"]);
    workspace.commit_at("Fix parser", &[("src/a.rs", "a\nfix\n")], at(2));
    workspace.rebuild();
    workspace.hotspots_json(&["--collect"]);

    workspace.git(&["checkout", "-q", "side"]);
    workspace.git(&["mv", "src/a.rs", "src/b.rs"]);
    workspace.git_at(&["commit", "-q", "-m", "Move parser"], at(3));
    let collect_pruning = |label: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&workspace.root)
            .env("AST_INDEX_DB_PATH", &workspace.db)
            .env("AST_INDEX_CACHE_DIR", &workspace.cache)
            .env("AST_INDEX_TEST_GIT_DEAD_COMMITS_LIMIT", "0")
            .args(["hotspots", "--collect", "--format", "json"])
            .output()
            .unwrap();
        assert_success(&output);
        workspace.assert_matches_fresh_collection(label);
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        json
    };
    collect_pruning("side, main's commit pruned");
    workspace.git(&["checkout", "-q", "main"]);
    let back = collect_pruning("main, re-read after pruning");
    // The pruned commit is gone from the store and has to be read again.
    assert_eq!(back["collection"]["commits_restored"], 0);
    assert_eq!(back["collection"]["commits_scanned"], 1);
    let conn = rusqlite::Connection::open(&workspace.db).unwrap();
    let dead: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM git_commits WHERE live = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dead, 0);
}

#[test]
fn history_from_an_older_version_is_rebuilt_once() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit("Add parser", &[("src/a.rs", "fn a() {}\n")]);
    workspace.rebuild();
    workspace.hotspots_json(&["--collect"]);

    // What an index collected before the per-commit store looks like: the
    // derived tables and the cursor, nothing else.
    let conn = rusqlite::Connection::open(&workspace.db).unwrap();
    conn.execute_batch(
        "DROP TABLE git_commit_changes; DROP TABLE git_commits; DROP TABLE git_paths;
         DELETE FROM metadata WHERE key IN ('git_signals_store', 'git_signals_paths');",
    )
    .unwrap();
    drop(conn);

    let report = workspace.hotspots_json(&[]);
    assert_eq!(find_item(&report, "src/a.rs")["commits"], 1);

    workspace.commit("Fix parser", &[("src/a.rs", "fn a() {}\nfn b() {}\n")]);
    let json = workspace.hotspots_json(&["--collect"]);
    assert_eq!(json["collection"]["mode"], "full");
    let reason = json["collection"]["reset_reason"].as_str().unwrap();
    assert!(reason.contains("older version"), "{reason}");
    assert_eq!(find_item(&json, "src/a.rs")["commits"], 2);
    workspace.collect_and_compare("the run after the upgrade");
}

/// A deterministic random walk over commits, renames, deletions, branch
/// switches, merges, rebases and resets; after every step the incrementally
/// maintained tables must equal a fresh full collection.
#[test]
fn random_history_walk_matches_a_fresh_full_collection() {
    let workspace = workspace();
    workspace.init_git();
    workspace.commit_at(
        "Add seed files",
        &[
            ("src/f0.rs", "0\n"),
            ("src/f1.rs", "1\n"),
            ("src/f2.rs", "2\n"),
        ],
        at(0),
    );
    workspace.rebuild();
    workspace.hotspots_json(&["--collect"]);

    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |bound: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % bound
    };
    let branches = ["main", "left", "right"];
    let mut created = vec!["main".to_string()];
    let mut serial = 3;
    let try_git = |args: &[&str], hour: i64| {
        let date = format!("@{} +0000", at(hour));
        git_command(&workspace.root, args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap()
            .status
            .success()
    };
    let tracked = || -> Vec<String> {
        let output = git_command(&workspace.root, &["ls-files", "src"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    };

    for step in 0..40i64 {
        let hour = 1 + step + (next(5) as i64) - 2;
        let files = tracked();
        let action = next(10);
        let label = match action {
            0..=2 if !files.is_empty() => {
                let file = &files[next(files.len() as u64) as usize];
                let path = workspace.root.join(file);
                let mut contents = fs::read_to_string(&path).unwrap_or_default();
                contents.push_str(&format!("line {step}\n"));
                fs::write(&path, contents).unwrap();
                workspace.git(&["add", file]);
                let subject = if next(2) == 0 { "Fix crash" } else { "Extend" };
                assert!(try_git(&["commit", "-q", "-m", subject], hour));
                format!("step {step}: edit {file}")
            }
            3 => {
                let file = format!("src/f{serial}.rs");
                serial += 1;
                workspace.commit_at("Add module", &[(file.as_str(), "new\n")], hour);
                format!("step {step}: add {file}")
            }
            4 if !files.is_empty() => {
                let file = files[next(files.len() as u64) as usize].clone();
                let target = format!("src/f{serial}.rs");
                serial += 1;
                workspace.git(&["mv", &file, &target]);
                assert!(try_git(&["commit", "-q", "-m", "Move module"], hour));
                format!("step {step}: rename {file} -> {target}")
            }
            5 if files.len() > 1 => {
                let file = files[next(files.len() as u64) as usize].clone();
                workspace.git(&["rm", "-q", &file]);
                assert!(try_git(&["commit", "-q", "-m", "Drop module"], hour));
                format!("step {step}: delete {file}")
            }
            6 | 7 => {
                let branch = branches[next(branches.len() as u64) as usize];
                if created.iter().any(|name| name == branch) {
                    workspace.git(&["checkout", "-q", branch]);
                } else {
                    workspace.git(&["checkout", "-q", "-b", branch]);
                    created.push(branch.to_string());
                }
                format!("step {step}: switch to {branch}")
            }
            8 => {
                let other = created[next(created.len() as u64) as usize].clone();
                if try_git(&["merge", "-q", "--no-edit", &other], hour) {
                    format!("step {step}: merge {other}")
                } else {
                    workspace.git(&["merge", "--abort"]);
                    format!("step {step}: merge {other} aborted")
                }
            }
            9 => {
                if next(2) == 0 && try_git(&["rev-parse", "-q", "--verify", "HEAD~1"], hour) {
                    workspace.git(&["reset", "-q", "--hard", "HEAD~1"]);
                    format!("step {step}: reset one commit")
                } else {
                    let other = created[next(created.len() as u64) as usize].clone();
                    if try_git(&["rebase", "-q", &other], hour) {
                        format!("step {step}: rebase onto {other}")
                    } else {
                        workspace.git(&["rebase", "--abort"]);
                        format!("step {step}: rebase onto {other} aborted")
                    }
                }
            }
            _ => continue,
        };
        workspace.collect_and_compare(&label);
    }
}
