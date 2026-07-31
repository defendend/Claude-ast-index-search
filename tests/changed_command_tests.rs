#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn write_fake_vcs(root: &Path, body: &str) -> PathBuf {
    let script = root.join("fake-vcs");
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    script
}

fn run_changed(cwd: &Path, fake_vcs: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(cwd)
        .env("AST_INDEX_VCS_BIN", fake_vcs)
        .args(["changed"])
        .args(args)
        .output()
        .expect("ast-index changed must run")
}

fn run_changed_with_home(cwd: &Path, fake_vcs: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(cwd)
        .env("AST_INDEX_VCS_BIN", fake_vcs)
        .env("HOME", home)
        .args(["changed"])
        .args(args)
        .output()
        .expect("ast-index changed must run")
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
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

#[test]
#[allow(deprecated)]
fn legacy_files_changed_api_signatures_remain_available() {
    let _: fn(&Path, &str) -> anyhow::Result<()> = ast_index::commands::files::cmd_changed;
    let _: fn(&Path) -> &'static str = ast_index::commands::files::detect_vcs;
    let _: fn(&Path) -> &'static str = ast_index::commands::files::detect_git_default_branch;
}

#[test]
#[allow(deprecated)]
fn legacy_vcs_detection_wrapper_delegates_to_changed_detection() {
    let temp = TempDir::new().unwrap();
    let arc_repo = temp.path().join("arc-repo");
    let git_repo = temp.path().join("git-repo");
    fs::create_dir_all(&arc_repo).unwrap();
    fs::create_dir_all(git_repo.join(".git")).unwrap();
    fs::write(arc_repo.join(".arcconfig"), "").unwrap();

    assert_eq!(ast_index::commands::files::detect_vcs(&arc_repo), "arc");
    assert_eq!(ast_index::commands::files::detect_vcs(&git_repo), "git");
}

#[test]
#[allow(deprecated)]
fn legacy_git_default_branch_wrapper_preserves_remote_head_mapping() {
    let temp = TempDir::new().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "user.email", "test@example.com"]);
    git(temp.path(), &["config", "user.name", "Test"]);
    fs::write(temp.path().join("tracked"), "content").unwrap();
    git(temp.path(), &["add", "tracked"]);
    git(temp.path(), &["commit", "-qm", "initial"]);
    git(
        temp.path(),
        &["update-ref", "refs/remotes/origin/master", "HEAD"],
    );
    git(
        temp.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/master",
        ],
    );

    assert_eq!(
        ast_index::commands::files::detect_git_default_branch(temp.path()),
        "origin/master"
    );
}

#[test]
fn changed_is_cache_independent_and_emits_json_v1() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let fake = write_fake_vcs(
        temp.path(),
        r#"printf 'A\0src/new.rs\0M\0src/mod.rs\0D\0src/old.rs\0R087\0src/before.rs\0src/after.rs\0C100\0src/source.rs\0src/copy.rs\0T\0src/type.rs\0'"#,
    );
    let cache = temp.path().join("cache-must-not-exist");

    let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(&repo)
        .env("AST_INDEX_VCS_BIN", &fake)
        .env("AST_INDEX_CACHE_DIR", &cache)
        .args(["changed", "--base", "origin/main", "--format", "json"])
        .output()
        .unwrap();

    assert_success(&output);
    assert!(
        !cache.exists(),
        "changed must not create or probe the index cache"
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["vcs"], "git");
    assert_eq!(json["base"], "origin/main");
    assert_eq!(json["head"], "HEAD");
    assert!(json["scope"].is_null());
    let changes = json["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 6);
    assert_eq!(changes[2]["status"], "D");
    assert_eq!(changes[2]["path"], "src/old.rs");
    assert_eq!(changes[3]["status"], "R");
    assert_eq!(changes[3]["old_path"], "src/before.rs");
    assert_eq!(changes[3]["path"], "src/after.rs");
    assert_eq!(changes[4]["status"], "A");
    assert!(changes[4].get("old_path").is_none());
    assert_eq!(changes[5]["status"], "M");
}

#[test]
fn changed_text_renders_deletion_and_rename_without_symbols() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let fake = write_fake_vcs(
        temp.path(),
        r#"printf 'D\0deleted.kt\0R100\0old.kt\0new.kt\0'"#,
    );

    let output = run_changed(&repo, &fake, &["--base", "main"]);
    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Changed files against main (2):\n  D  deleted.kt\n  R  old.kt -> new.kt\n"
    );
}

#[test]
fn changed_filters_to_invocation_directory_scope() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let scope = repo.join("nested/deep");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&scope).unwrap();
    let argv_log = temp.path().join("git-argv");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            r#"printf '%s\n' "$@" > '{}'
printf 'M\0outside.rs\0A\0nested/deep/inside.rs\0R100\0nested/deep/old.rs\0outside-new.rs\0'"#,
            argv_log.display()
        ),
    );

    let output = run_changed(
        &scope,
        &fake,
        &["--base", "origin/main", "--format", "json", "--local"],
    );
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        "diff\n--merge-base\n--name-status\n-z\n-M\n--no-ext-diff\n--no-textconv\norigin/main\nHEAD\n--\nnested/deep\n"
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["scope"], "nested/deep");
    let changes = json["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["path"], "nested/deep/inside.rs");
    assert_eq!(changes[1]["old_path"], "nested/deep/old.rs");
    assert_eq!(changes[1]["path"], "outside-new.rs");
}

#[test]
fn changed_uses_native_arc_scope_and_restores_repo_relative_paths() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let scope = repo.join("nested/deep");
    fs::create_dir_all(&scope).unwrap();
    fs::write(repo.join(".arcconfig"), "").unwrap();
    let cwd_log = temp.path().join("arc-cwd");
    let argv_log = temp.path().join("arc-argv");
    let canonical_scope = scope.canonicalize().unwrap();
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            r#"pwd -P > '{}'
printf '%s\n' "$@" > '{}'
if [ "$(pwd -P)" = '{}' ] &&
   [ "$#" -eq 5 ] &&
   [ "$1" = diff ] &&
   [ "$2" = -B ] &&
   [ "$3" = --name-status ] &&
   [ "$4" = --no-color ] &&
   [ "$5" = --relative=. ]; then
    printf 'A\tadded.rs\nM\tmodified.rs\nD\tdeleted.rs\nR100\told.rs\trenamed.rs\n'
else
    printf 'M\toutside.rs\n'
fi"#,
            cwd_log.display(),
            argv_log.display(),
            canonical_scope.display()
        ),
    );

    let output = run_changed(&scope, &fake, &["--format", "json", "--local"]);
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(cwd_log).unwrap(),
        format!("{}\n", canonical_scope.display())
    );
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        "diff\n-B\n--name-status\n--no-color\n--relative=.\n"
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let changes = json["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 4);
    assert_eq!(changes[0]["status"], "A");
    assert_eq!(changes[0]["path"], "nested/deep/added.rs");
    assert_eq!(changes[1]["status"], "M");
    assert_eq!(changes[1]["path"], "nested/deep/modified.rs");
    assert_eq!(changes[2]["status"], "D");
    assert_eq!(changes[2]["path"], "nested/deep/deleted.rs");
    assert_eq!(changes[3]["status"], "R");
    assert_eq!(changes[3]["old_path"], "nested/deep/old.rs");
    assert_eq!(changes[3]["path"], "nested/deep/renamed.rs");
    assert!(
        changes.iter().all(|change| change["path"] != "outside.rs"),
        "native Arc scoping must prevent the outside fixture from reaching output"
    );
}

#[test]
fn changed_arc_root_keeps_full_branch_diff() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join(".arcconfig"), "").unwrap();
    let cwd_log = temp.path().join("arc-root-cwd");
    let argv_log = temp.path().join("arc-root-argv");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            r#"pwd -P > '{}'
printf '%s\n' "$@" > '{}'
printf 'M\toutside.rs\nA\tnested/inside.rs\n'"#,
            cwd_log.display(),
            argv_log.display()
        ),
    );

    let output = run_changed(&repo, &fake, &["--format", "json"]);
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(cwd_log).unwrap(),
        format!("{}\n", repo.canonicalize().unwrap().display())
    );
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        "diff\n-B\n--name-status\n--no-color\n"
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["scope"].is_null());
    let changes = json["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["path"], "outside.rs");
    assert_eq!(changes[1]["path"], "nested/inside.rs");
}

#[test]
fn changed_timeout_is_nonzero_and_keeps_stdout_empty() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let fake = write_fake_vcs(temp.path(), "sleep 2\nprintf 'M\\0late.rs\\0'");

    let output = run_changed(
        &repo,
        &fake,
        &["--base", "origin/main", "--timeout-ms", "40"],
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("timed out after 40ms"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn changed_timeout_covers_pipe_drain_and_kills_background_group() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let survived = temp.path().join("background-survived");
    let ready = temp.path().join("background-ready");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            "(trap '' HUP; touch '{}'; sleep 0.25; touch '{}') &\nwhile [ ! -e '{}' ]; do :; done\nexit 0",
            ready.display(),
            survived.display(),
            ready.display()
        ),
    );

    let started = Instant::now();
    let output = run_changed(
        &repo,
        &fake,
        &["--base", "origin/main", "--timeout-ms", "60"],
    );
    let elapsed = started.elapsed();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("timed out after 60ms"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        elapsed < Duration::from_millis(180),
        "pipe drain exceeded wall-clock deadline: {elapsed:?}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !survived.exists(),
        "timed-out descendants must be killed with the VCS process group"
    );
}

#[test]
fn changed_nonzero_vcs_exit_is_an_error_and_keeps_stdout_empty() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let fake = write_fake_vcs(temp.path(), "echo 'bad base' >&2\nexit 23");

    let output = run_changed(&repo, &fake, &["--base", "origin/main"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exited with"), "stderr={stderr}");
    assert!(stderr.contains("bad base"), "stderr={stderr}");
}

#[test]
fn changed_rejects_subtree_and_unsafe_base_before_vcs() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let marker = temp.path().join("vcs-was-called");
    let fake = write_fake_vcs(
        temp.path(),
        &format!("touch '{}'\nprintf ''", marker.display()),
    );

    let subtree = run_changed(&repo, &fake, &["--subtree", "extra"]);
    assert!(!subtree.status.success());
    assert!(subtree.stdout.is_empty());
    assert!(String::from_utf8_lossy(&subtree.stderr).contains("--subtree is not supported"));
    assert!(!marker.exists());

    let unsafe_base = run_changed(&repo, &fake, &["--base=-option"]);
    assert!(!unsafe_base.status.success());
    assert!(unsafe_base.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsafe_base.stderr).contains("must not start"));
    assert!(!marker.exists());
}

#[test]
fn changed_accepts_git_file_marker() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join(".git"), "gitdir: ../metadata\n").unwrap();
    let fake = write_fake_vcs(temp.path(), "printf ''");

    let output = run_changed(&repo, &fake, &["--base", "origin/main"]);
    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Changed files against origin/main (0):\n"
    );
}

#[test]
fn changed_resolves_origin_head_before_diff() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let calls = temp.path().join("calls");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            r#"printf '%s|' "$@" >> '{}'
printf '\n' >> '{}'
if [ "$1" = symbolic-ref ]; then
    printf 'origin/release\n'
    exit 0
fi
if [ "$1" = diff ] && [ "$8" = origin/release ] && [ "$9" = HEAD ]; then
    printf 'M\0release.rs\0'
    exit 0
fi
exit 91"#,
            calls.display(),
            calls.display()
        ),
    );

    let output = run_changed(&repo, &fake, &["--format", "json"]);
    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["base"], "origin/release");
    assert_eq!(json["changes"][0]["path"], "release.rs");
    assert_eq!(
        fs::read_to_string(calls).unwrap(),
        "symbolic-ref|--quiet|--short|refs/remotes/origin/HEAD|\ndiff|--merge-base|--name-status|-z|-M|--no-ext-diff|--no-textconv|origin/release|HEAD|\n"
    );
}

#[test]
fn changed_falls_back_to_origin_master_without_symbolic_remote() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let calls = temp.path().join("calls");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            r#"printf '%s|' "$@" >> '{}'
printf '\n' >> '{}'
if [ "$1" = symbolic-ref ]; then
    exit 1
fi
if [ "$1" = rev-parse ] && [ "$4" = 'origin/master^{{commit}}' ]; then
    exit 0
fi
if [ "$1" = diff ] && [ "$8" = origin/master ]; then
    printf 'M\0master.rs\0'
    exit 0
fi
exit 1"#,
            calls.display(),
            calls.display()
        ),
    );

    let output = run_changed(&repo, &fake, &["--format", "json"]);
    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["base"], "origin/master");
    assert_eq!(json["changes"][0]["path"], "master.rs");
    let calls = fs::read_to_string(calls).unwrap_or_default();
    assert!(calls.contains("rev-parse|--verify|--quiet|origin/main^{commit}|"));
    assert!(calls.contains("rev-parse|--verify|--quiet|origin/master^{commit}|"));
}

#[test]
fn changed_git_base_probes_share_the_diff_wall_clock_deadline() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let diff_called = temp.path().join("diff-called");
    let calls = temp.path().join("calls");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            r#"printf '%s\n' "$1" >> '{}'
if [ "$1" = diff ]; then
    touch '{}'
    exit 0
fi
sleep 0.05
exit 1"#,
            calls.display(),
            diff_called.display()
        ),
    );

    let output = run_changed(&repo, &fake, &["--timeout-ms", "80"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("timed out after 80ms"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(calls).unwrap_or_default();
    assert!(
        calls.lines().count() <= 2,
        "Git probes reset the wall-clock deadline: {calls:?}"
    );
    if let Some(first) = calls.lines().next() {
        assert_eq!(first, "symbolic-ref");
    }
    assert!(!diff_called.exists());
}

#[test]
fn changed_real_git_falls_back_to_local_master_without_origin() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "master"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Test User"]);
    fs::write(repo.join("tracked.rs"), "fn before() {}\n").unwrap();
    git(&repo, &["add", "tracked.rs"]);
    git(&repo, &["commit", "-m", "initial"]);
    git(&repo, &["checkout", "-b", "feature"]);
    fs::write(repo.join("tracked.rs"), "fn after() {}\n").unwrap();
    git(&repo, &["add", "tracked.rs"]);
    git(&repo, &["commit", "-m", "change"]);

    let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(&repo)
        .args(["changed", "--format", "json"])
        .output()
        .unwrap();
    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["base"], "master");
    assert_eq!(json["changes"][0]["status"], "M");
    assert_eq!(json["changes"][0]["path"], "tracked.rs");
}

#[test]
fn changed_ignores_home_level_arc_markers_outside_a_repository() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let cwd = home.join("outside/project");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(home.join(".arcconfig"), "global config\n").unwrap();
    fs::create_dir_all(home.join(".arc")).unwrap();
    fs::write(home.join(".arc/HEAD"), "trunk\n").unwrap();
    let called = temp.path().join("called");
    let fake = write_fake_vcs(
        temp.path(),
        &format!("touch '{}'\nprintf ''", called.display()),
    );

    let output = run_changed_with_home(&cwd, &fake, &home, &["--base", "trunk"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no Git or Arc working tree found"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!called.exists());
}

#[test]
fn changed_normalizes_legacy_arc_origin_base() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join(".arcconfig"), "").unwrap();
    let argv_log = temp.path().join("arc-argv");
    let fake = write_fake_vcs(
        temp.path(),
        &format!(
            "printf '%s\\n' \"$@\" > '{}'\nprintf 'M\\tfile.rs\\n'",
            argv_log.display()
        ),
    );

    let output = run_changed(
        &repo,
        &fake,
        &["--base", "origin/feature", "--format", "json"],
    );
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        "diff\n-B\n--name-status\n--no-color\nfeature\nHEAD\n"
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["base"], "feature");
}

#[test]
fn changed_text_escapes_controls_but_json_preserves_paths_and_unicode() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let fake = write_fake_vcs(
        temp.path(),
        r#"printf 'M\0line
tab	Юникод.rs\0R100\0old
name.rs\0new	name.rs\0'"#,
    );

    let text = run_changed(&repo, &fake, &["--base", "main"]);
    assert_success(&text);
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "Changed files against main (2):\n  M  line\\ntab\\tЮникод.rs\n  R  old\\nname.rs -> new\\tname.rs\n"
    );

    let json_output = run_changed(&repo, &fake, &["--base", "main", "--format", "json"]);
    assert_success(&json_output);
    let rendered = String::from_utf8(json_output.stdout.clone()).unwrap();
    assert!(rendered.contains("Юникод.rs"));
    let json: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(json["changes"][0]["path"], "line\ntab\tЮникод.rs");
    assert_eq!(json["changes"][1]["old_path"], "old\nname.rs");
    assert_eq!(json["changes"][1]["path"], "new\tname.rs");
}

#[test]
fn changed_verbose_prints_debug_safe_cwd_executable_and_exact_argv() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let scope = repo.join("nested\nline");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&scope).unwrap();
    let fake = write_fake_vcs(temp.path(), "printf ''");

    let output = run_changed(&scope, &fake, &["--base", "main", "--verbose", "--local"]);
    assert_success(&output);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cwd=\""), "stderr={stderr}");
    assert!(stderr.contains("nested\\nline\""), "stderr={stderr}");
    assert!(
        stderr.contains(&format!("executable={:?}", fake.as_os_str())),
        "stderr={stderr}"
    );
    assert!(
        stderr.contains(
            "argv=[\"diff\", \"--merge-base\", \"--name-status\", \"-z\", \"-M\", \"--no-ext-diff\", \"--no-textconv\", \"main\", \"HEAD\", \"--\", \"nested\\nline\"]"
        ),
        "stderr={stderr}"
    );
}
