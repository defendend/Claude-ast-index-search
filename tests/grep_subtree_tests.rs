//! `callers` must walk attached subtrees, not only the primary root. Before
//! this, `--subtree` was advertised on it but silently ignored: a caller living
//! in a subtree was never found.

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

/// Sibling folders with a common ancestor, like `util/` and `search/` under
/// a monorepo root: the primary index lives in `search/`, `util/` is attached.
fn workspace() -> (TempDir, TempDir) {
    let tmp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let search = tmp.path().join("search");
    let util = tmp.path().join("util");
    write(&search.join("go.mod"), "module search\n");
    write(
        &search.join("s.go"),
        "package search\n\nfunc SearchMain() { UtilHelper() }\n",
    );
    write(&util.join("go.mod"), "module util\n");
    write(
        &util.join("u.go"),
        "package util\n\nfunc UtilHelper() {}\n\nfunc UtilCaller() { UtilHelper() }\n",
    );
    write(
        &util.join("todo.go"),
        "package util\n\n// TODO: subtree marker\n",
    );
    assert!(run(&search, cache.path(), &["rebuild"]).status.success());
    assert!(run(
        &search,
        cache.path(),
        &["subtree", "add", "util", "../util"]
    )
    .status
    .success());
    assert!(run(&search, cache.path(), &["rebuild"]).status.success());
    (tmp, cache)
}

#[test]
fn callers_finds_call_sites_inside_attached_subtree() {
    let (tmp, cache) = workspace();
    let search = tmp.path().join("search");
    let out = stdout(&run(&search, cache.path(), &["callers", "UtilHelper"]));
    assert!(out.contains("s.go"), "primary caller missing: {out}");
    assert!(
        out.contains("[util]"),
        "subtree caller not decorated: {out}"
    );
    assert!(out.contains("u.go"), "subtree caller missing: {out}");
    assert!(
        !out.contains(".."),
        "subtree path leaked as relative: {out}"
    );
}

#[test]
fn callers_subtree_flag_restricts_to_that_subtree() {
    let (tmp, cache) = workspace();
    let search = tmp.path().join("search");
    let out = stdout(&run(
        &search,
        cache.path(),
        &["callers", "UtilHelper", "--subtree", "util"],
    ));
    assert!(out.contains("u.go"), "{out}");
    assert!(
        !out.contains("s.go"),
        "primary must be excluded under --subtree: {out}"
    );
}

#[test]
fn callers_local_flag_excludes_subtrees() {
    let (tmp, cache) = workspace();
    let search = tmp.path().join("search");
    let out = stdout(&run(
        &search,
        cache.path(),
        &["callers", "UtilHelper", "--local"],
    ));
    assert!(out.contains("s.go"), "{out}");
    assert!(
        !out.contains("u.go"),
        "subtree must be excluded under --local: {out}"
    );
}
