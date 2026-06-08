//! Step 4 of #31: `--subtree NAME` and `--local` narrow search results to
//! a single subtree (or to the primary project only).

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
    write(&main.join("src/lib.rs"), "pub fn shared_fn() {}\n");
    write(&extra.join("Cargo.toml"), "[package]\nname=\"e\"\nversion=\"0\"\n");
    write(&extra.join("src/lib.rs"), "pub fn shared_fn() {}\n");

    assert!(run(&main, &["rebuild"]).status.success());
    assert!(run(&main, &["subtree", "add", "extra", "../extra"]).status.success());
    assert!(run(&main, &["rebuild"]).status.success());

    let main_path = main.clone();
    (tmp, main_path)
}

fn parse_json(stdout: &[u8]) -> serde_json::Value {
    serde_json::from_slice(stdout).expect("expected valid JSON output")
}

#[test]
fn no_filter_returns_both() {
    let (_tmp, main) = make_workspace();
    let out = run(&main, &["--format", "json", "symbol", "shared_fn"]);
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    let names: Vec<&str> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "both copies should be found: {names:?}");
}

#[test]
fn local_flag_keeps_only_primary() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &["--local", "--format", "json", "symbol", "shared_fn"],
    );
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1, "--local must drop the subtree row");
    let path = arr[0]["path"].as_str().unwrap();
    assert!(path.contains("/main/"), "expected primary path, got {path}");
}

#[test]
fn subtree_flag_keeps_only_named_subtree() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &[
            "--subtree", "extra", "--format", "json", "symbol", "shared_fn",
        ],
    );
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1, "--subtree extra must keep only extra rows");
    let path = arr[0]["path"].as_str().unwrap();
    assert!(path.contains("/extra/"), "expected subtree path, got {path}");
}

#[test]
fn subtree_flag_with_unknown_name_returns_empty() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &[
            "--subtree", "ghost", "--format", "json", "symbol", "shared_fn",
        ],
    );
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    assert!(parsed.as_array().unwrap().is_empty());
}

#[test]
fn local_and_subtree_conflict() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &["--local", "--subtree", "extra", "symbol", "shared_fn"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mutually exclusive"),
        "expected conflict error, got: {stderr}"
    );
}
