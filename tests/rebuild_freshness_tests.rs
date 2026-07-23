use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rusqlite::Connection;
use tempfile::TempDir;

fn run(project: &Path, cache: &Path, args: &[&str], max_files: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(project)
        .args(args)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("AST_INDEX_MAX_FILES", max_files)
        .output()
        .unwrap()
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn live_db_path(project: &Path, cache: &Path) -> PathBuf {
    let output = run(project, cache, &["db-path"], "0");
    assert_success("db-path", &output);
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn metadata_value(conn: &Connection, key: &str) -> String {
    conn.query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn rebuild_records_freshness_only_after_successful_swap_candidate() {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    fs::write(project.path().join("one.rs"), "pub struct One;\n").unwrap();
    fs::write(project.path().join("two.rs"), "pub struct Two;\n").unwrap();

    let rebuilt = run(project.path(), cache.path(), &["rebuild"], "0");
    assert_success("initial rebuild", &rebuilt);

    let db_path = live_db_path(project.path(), cache.path());
    let conn = Connection::open(&db_path).unwrap();
    let updated = metadata_value(&conn, "last_update_at")
        .parse::<i64>()
        .unwrap();
    let modules = metadata_value(&conn, "last_modules_indexed_at")
        .parse::<i64>()
        .unwrap();
    assert!(updated > 0);
    assert!(modules >= updated);

    conn.execute(
        "UPDATE metadata SET value = '7' WHERE key = 'last_update_at'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE metadata SET value = '11' WHERE key = 'last_modules_indexed_at'",
        [],
    )
    .unwrap();
    drop(conn);

    let failed = run(project.path(), cache.path(), &["rebuild"], "1");
    assert!(
        !failed.status.success(),
        "candidate rebuild unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("walker stopped"),
        "unexpected failure: {}",
        String::from_utf8_lossy(&failed.stderr)
    );

    let conn = Connection::open(&db_path).unwrap();
    assert_eq!(metadata_value(&conn, "last_update_at"), "7");
    assert_eq!(metadata_value(&conn, "last_modules_indexed_at"), "11");
}
