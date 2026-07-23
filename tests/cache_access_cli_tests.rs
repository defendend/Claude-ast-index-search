use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ast-index"))
}

fn run(cwd: &Path, cache_path: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(cwd)
        .args(args)
        .env("AST_INDEX_CACHE_DIR", cache_path)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .output()
        .unwrap()
}

#[test]
fn cache_independent_commands_skip_invalid_cache_but_index_commands_fail() {
    let tmp = TempDir::new().unwrap();
    let untouched_cache = tmp.path().join("untouched-cache");
    let invalid_cache = tmp.path().join("cache-is-a-file");
    fs::write(&invalid_cache, "not a directory").unwrap();

    let version = run(tmp.path(), &untouched_cache, &["version"]);
    assert!(version.status.success());
    assert!(
        !untouched_cache.exists(),
        "version must not create or lock the cache layout"
    );

    let version = run(tmp.path(), &invalid_cache, &["version"]);
    assert!(
        version.status.success(),
        "version unexpectedly accessed the cache: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(String::from_utf8_lossy(&version.stdout).contains("ast-index v"));

    let todo = run(tmp.path(), &invalid_cache, &["todo"]);
    assert!(!todo.status.success(), "todo must report an unusable cache");
    let stderr = String::from_utf8_lossy(&todo.stderr);
    assert!(
        stderr.contains("cache") || stderr.contains("directory"),
        "todo returned the wrong error: {stderr}"
    );
}
