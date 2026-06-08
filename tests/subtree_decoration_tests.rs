//! Step 3 of #31: when a file belongs to a named subtree, text-mode
//! output gets prefixed with `[name] /abs/path`. JSON-mode output stays
//! raw so downstream tools don't have to parse the prefix.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ast-index"))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn run(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(cwd)
        .args(args)
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap()
}

fn make_workspace() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("main");
    let extra = tmp.path().join("extra");
    write(&main.join("Cargo.toml"), "[package]\nname=\"m\"\nversion=\"0\"\n");
    write(&main.join("src/lib.rs"), "pub fn main_only_fn() {}\n");
    write(&extra.join("Cargo.toml"), "[package]\nname=\"e\"\nversion=\"0\"\n");
    write(&extra.join("src/lib.rs"), "pub fn extra_only_fn() {}\n");

    let out = run(&main, &["rebuild"]);
    assert!(out.status.success());
    let out = run(&main, &["subtree", "add", "extra", "../extra"]);
    assert!(out.status.success());
    let out = run(&main, &["rebuild"]);
    assert!(out.status.success());

    let main_path = main.clone();
    // tmp must outlive main_path — keep both in scope.
    (tmp, main_path)
}

#[test]
fn text_mode_prefixes_subtree_paths() {
    let (_tmp, main) = make_workspace();

    let out = run(&main, &["symbol", "extra_only_fn"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[extra]"),
        "expected [extra] prefix, got: {stdout}"
    );
    assert!(stdout.contains("/extra/src/lib.rs"));
}

#[test]
fn text_mode_does_not_prefix_primary_paths() {
    let (_tmp, main) = make_workspace();

    let out = run(&main, &["symbol", "main_only_fn"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The string `[function]` is the symbol kind label printed by the
    // symbol command; we care about the subtree prefix only — `[extra] /…`
    // or `[main] /…` or any `[<name>] /` pattern at the start of the path.
    assert!(
        !stdout.contains("[extra]") && !stdout.contains("[main]"),
        "primary-project paths must not get a subtree prefix, got: {stdout}"
    );
    assert!(stdout.contains("/main/src/lib.rs"));
}

#[test]
fn json_mode_keeps_raw_path_without_prefix() {
    let (_tmp, main) = make_workspace();

    let out = run(&main, &["--format", "json", "symbol", "extra_only_fn"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let path = parsed[0]["path"].as_str().unwrap();
    assert!(
        !path.starts_with("["),
        "JSON path must be raw, got: {path}"
    );
    assert!(path.contains("/extra/src/lib.rs"));
}
