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
}

fn git_in(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Dev One")
        .env("GIT_AUTHOR_EMAIL", "dev@example.invalid")
        .env("GIT_COMMITTER_NAME", "Dev One")
        .env("GIT_COMMITTER_EMAIL", "dev@example.invalid")
        .args(args)
        .output()
        .expect("git must run");
    assert!(
        output.status.success(),
        "git {:?}: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
fn rewritten_history_forces_a_clean_recollect() {
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
    fs::remove_file(workspace.root.join("src/throwaway.rs")).ok();

    let recollected = workspace.hotspots_json(&["--collect"]);
    assert_eq!(recollected["collection"]["mode"], "full");
    assert_eq!(recollected["collection"]["commits_scanned"], 1);
    assert_ne!(recollected["head"], discarded_head.as_str());
    let reason = recollected["collection"]["reset_reason"]
        .as_str()
        .expect("a discarded cursor must be explained");
    assert!(reason.contains("no longer an ancestor"), "{reason}");
    assert_eq!(recollected["files_with_history"], 1);
    assert_eq!(recollected["items"].as_array().unwrap().len(), 1);
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
fn deleted_files_keep_their_row_but_drop_out_of_the_report() {
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
    // The row survives for a later resurrection, but a deleted path is not
    // part of the population percentiles are computed against.
    assert_eq!(json["paths_in_history"], 2);
    assert_eq!(json["files_with_history"], 1);
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
