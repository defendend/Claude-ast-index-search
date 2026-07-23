use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

const ACTIVITY_MARKER: &str = ".ast-index-access-v1";

fn run_ast_index(project_root: &Path, cache_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .args(args)
        .current_dir(project_root)
        .env("AST_INDEX_CACHE_DIR", cache_dir)
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("ast-index command should start")
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    value.into()
}

fn db_files(db_path: &Path) -> [PathBuf; 3] {
    [
        db_path.to_path_buf(),
        append_suffix(db_path, "-wal"),
        append_suffix(db_path, "-shm"),
    ]
}

fn effective_mtime(paths: &[PathBuf]) -> SystemTime {
    paths
        .iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .max()
        .expect("index.db or a SQLite sidecar should exist")
}

#[test]
fn indexed_usages_refreshes_external_activity_without_writing_database() {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    fs::write(
        project.path().join("UsageActivitySentinel.kt"),
        "class UsageActivitySentinel\n",
    )
    .unwrap();
    let usage_path = project.path().join("Consumer.kt");
    fs::write(
        &usage_path,
        "fun consume(value: UsageActivitySentinel) = value.toString()\n",
    )
    .unwrap();

    let rebuild = run_ast_index(project.path(), cache.path(), &["rebuild"]);
    assert!(
        rebuild.status.success(),
        "rebuild failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&rebuild.stdout),
        String::from_utf8_lossy(&rebuild.stderr)
    );

    let db_path_output = run_ast_index(project.path(), cache.path(), &["db-path"]);
    assert!(db_path_output.status.success());
    let db_path = PathBuf::from(String::from_utf8(db_path_output.stdout).unwrap().trim());
    let activity_marker = db_path.parent().unwrap().join(ACTIVITY_MARKER);
    assert!(activity_marker.is_file());

    // Remove the source usage so the assertion below can only be satisfied
    // by the indexed reference, never by cmd_usages' grep fallback.
    fs::remove_file(&usage_path).unwrap();

    let files = db_files(&db_path);
    let stale = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
    for path in files.iter().filter(|path| path.exists()) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(stale)
            .unwrap();
    }
    fs::OpenOptions::new()
        .write(true)
        .open(&activity_marker)
        .unwrap()
        .set_modified(stale)
        .unwrap();
    let fresh_cutoff = SystemTime::now() - Duration::from_secs(5);
    assert!(effective_mtime(&files) < fresh_cutoff);

    let usages = run_ast_index(
        project.path(),
        cache.path(),
        &["usages", "UsageActivitySentinel", "--limit", "5"],
    );
    assert!(
        usages.status.success(),
        "usages failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&usages.stdout),
        String::from_utf8_lossy(&usages.stderr)
    );
    let stdout = String::from_utf8_lossy(&usages.stdout);
    assert!(
        stdout.contains("Consumer.kt:1"),
        "expected indexed usage in output, got:\n{stdout}"
    );

    let touched = fs::metadata(&activity_marker).unwrap().modified().unwrap();
    assert!(
        touched >= fresh_cutoff,
        "expected external activity marker to be refreshed; got {touched:?}"
    );
    assert!(
        effective_mtime(&files) < fresh_cutoff,
        "read command unexpectedly wrote the database family"
    );
}
