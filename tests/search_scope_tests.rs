//! Content search walks what the index covers: attached subtrees are scanned,
//! `--local` / `--subtree` narrow the walk, and the `.ast-index.yaml`
//! `include` / `exclude` settings keep `search` (and the other grep-based
//! commands) out of directories the indexer skips.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run(cwd: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(cwd)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .args(args)
        .output()
        .unwrap()
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn rebuild(root: &Path, cache: &Path) {
    let out = run(root, cache, &["rebuild", "--no-deps"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn search_skips_config_excluded_directories() {
    let tmp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join(".ast-index.yaml"), "exclude:\n  - excluded\n");
    write(&root.join("package.json"), "{\"name\": \"p\"}\n");
    write(&root.join("included/Visible.js"), "export class VisibleScopeProbe {}\n");
    write(&root.join("excluded/Hidden.js"), "export class ExcludedScopeProbe {}\n");
    rebuild(root, cache.path());

    let out = stdout(&run(root, cache.path(), &["search", "ScopeProbe"]));
    assert!(out.contains("Visible.js"), "{out}");
    assert!(!out.contains("Hidden.js"), "excluded dir was scanned: {out}");
}

#[test]
fn todo_skips_config_excluded_directories() {
    let tmp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join(".ast-index.yaml"), "exclude:\n  - vendor/\n");
    write(&root.join("package.json"), "{\"name\": \"p\"}\n");
    write(&root.join("src/a.js"), "// TODO: own marker\n");
    write(&root.join("vendor/lib/b.js"), "// TODO: vendored marker\n");
    rebuild(root, cache.path());

    let out = stdout(&run(root, cache.path(), &["todo"]));
    assert!(out.contains("own marker"), "{out}");
    assert!(!out.contains("vendored marker"), "excluded dir was scanned: {out}");
}

#[test]
fn search_walks_only_config_included_directories() {
    let tmp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join(".ast-index.yaml"), "include:\n  - src\n  - missing\n");
    write(&root.join("package.json"), "{\"name\": \"p\"}\n");
    write(&root.join("src/a.js"), "export const IncludeProbe = 1;\n");
    write(&root.join("big/b.js"), "export const IncludeProbe = 2;\n");
    rebuild(root, cache.path());

    let out = run(root, cache.path(), &["search", "IncludeProbe"]);
    assert!(out.status.success(), "a missing include dir must not fail search");
    let out = stdout(&out);
    assert!(out.contains("src/a.js"), "{out}");
    assert!(!out.contains("big/b.js"), "dir outside include was scanned: {out}");
}

#[test]
fn search_respects_gitignore_inside_included_directories() {
    let tmp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join(".gitignore"), "generated/\n");
    write(&root.join(".ast-index.yaml"), "include:\n  - src\n");
    write(&root.join("package.json"), "{\"name\": \"p\"}\n");
    write(&root.join("src/a.js"), "export const IgnoreProbe = 1;\n");
    write(&root.join("src/generated/g.js"), "export const IgnoreProbe = 2;\n");
    rebuild(root, cache.path());

    let out = stdout(&run(root, cache.path(), &["search", "IgnoreProbe"]));
    assert!(out.contains("src/a.js"), "{out}");
    assert!(!out.contains("g.js"), "gitignored file was scanned: {out}");
}

/// The primary index lives in `main/`, `util/` is attached as a subtree.
fn subtree_workspace() -> (TempDir, TempDir) {
    let tmp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let main = tmp.path().join("main");
    let util = tmp.path().join("util");
    write(&main.join("go.mod"), "module main\n");
    write(
        &main.join("m.go"),
        "package main\n\nfunc MainSide() { SharedNeedle() }\n",
    );
    write(&util.join("go.mod"), "module util\n");
    write(&util.join("u.go"), "package util\n\nfunc SharedNeedle() {}\n");
    assert!(run(&main, cache.path(), &["rebuild"]).status.success());
    assert!(run(&main, cache.path(), &["subtree", "add", "util", "../util"])
        .status
        .success());
    (tmp, cache)
}

#[test]
fn search_scans_content_of_attached_subtree() {
    let (tmp, cache) = subtree_workspace();
    let main = tmp.path().join("main");
    let out = stdout(&run(&main, cache.path(), &["search", "SharedNeedle"]));
    let content = out.split("Content matches").nth(1).unwrap_or_default();
    assert!(content.contains("m.go"), "{out}");
    assert!(content.contains("u.go"), "subtree content missing: {out}");
}

#[test]
fn search_local_and_subtree_flags_narrow_content_scan() {
    let (tmp, cache) = subtree_workspace();
    let main = tmp.path().join("main");

    let local = stdout(&run(&main, cache.path(), &["search", "SharedNeedle", "--local"]));
    let local = local.split("Content matches").nth(1).unwrap_or_default();
    assert!(local.contains("m.go"), "{local}");
    assert!(!local.contains("u.go"), "subtree scanned under --local: {local}");

    let only = stdout(&run(
        &main,
        cache.path(),
        &["search", "SharedNeedle", "--subtree", "util"],
    ));
    let only = only.split("Content matches").nth(1).unwrap_or_default();
    assert!(only.contains("u.go"), "{only}");
    assert!(!only.contains("m.go"), "primary scanned under --subtree: {only}");
}
