use std::fs;
use std::path::Path;
use std::process::Command;

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn metadata_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .ok()
}

fn has_file(conn: &Connection, path: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
        [path],
        |row| row.get(0),
    )
    .unwrap()
}

fn has_symbol(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM symbols WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )
    .unwrap()
}

fn seed_freshness(conn: &Connection) {
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_update_at', '7')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_modules_indexed_at', '11')",
        [],
    )
    .unwrap();
}

#[test]
fn deletion_committed_before_changed_insert_failure_remains_stale() {
    let project = TempDir::new().unwrap();
    let changed = project.path().join("src/changed.rs");
    let deleted = project.path().join("src/deleted.rs");
    write_file(&changed, "pub struct Before;\n");
    write_file(&deleted, "pub struct Deleted;\n");

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();
    seed_freshness(&conn);

    write_file(&changed, "pub struct AfterWithLongerName;\n");
    fs::remove_file(&deleted).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_file_insert BEFORE INSERT ON files
         BEGIN SELECT RAISE(ABORT, 'forced changed insert failure'); END;",
    )
    .unwrap();

    let error = indexer::update_directory_incremental(&mut conn, project.path(), false, None, None)
        .unwrap_err();

    assert!(format!("{error:#}").contains("forced changed insert failure"));
    assert!(
        !has_file(&conn, "src/deleted.rs"),
        "deletion transaction must already be committed"
    );
    assert!(
        metadata_value(&conn, "index_update_dirty_at").is_some(),
        "partial update must retain a durable dirty marker"
    );
    let (modules_indexed_at, effective_update_at) =
        db::get_modules_index_freshness(&conn).unwrap().unwrap();
    assert!(
        effective_update_at > modules_indexed_at,
        "module consumer must treat the partial update as stale"
    );
}

#[test]
fn committed_data_with_failed_completion_recovers_on_next_noop() {
    let project = TempDir::new().unwrap();
    let changed = project.path().join("src/changed.rs");
    write_file(&changed, "pub struct Before;\n");

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();
    seed_freshness(&conn);
    conn.execute_batch(
        "CREATE TRIGGER fail_update_completion BEFORE UPDATE OF value ON metadata
         WHEN OLD.key = 'last_update_at'
         BEGIN SELECT RAISE(ABORT, 'forced metadata completion failure'); END;",
    )
    .unwrap();

    write_file(&changed, "pub struct AfterWithLongerName;\n");
    let error = indexer::update_directory_incremental(&mut conn, project.path(), false, None, None)
        .unwrap_err();

    assert!(format!("{error:#}").contains("forced metadata completion failure"));
    assert!(has_symbol(&conn, "AfterWithLongerName"));
    assert!(!has_symbol(&conn, "Before"));
    assert_eq!(
        metadata_value(&conn, "last_update_at").as_deref(),
        Some("7")
    );
    assert!(metadata_value(&conn, "index_update_dirty_at").is_some());
    let (modules_indexed_at, effective_update_at) =
        db::get_modules_index_freshness(&conn).unwrap().unwrap();
    assert!(effective_update_at > modules_indexed_at);

    conn.execute("DROP TRIGGER fail_update_completion", [])
        .unwrap();
    let result =
        indexer::update_directory_incremental(&mut conn, project.path(), false, None, None)
            .unwrap();

    assert_eq!(result, (0, 0, 0));
    assert_eq!(metadata_value(&conn, "index_update_dirty_at"), None);
    assert!(
        metadata_value(&conn, "last_update_at")
            .unwrap()
            .parse::<i64>()
            .unwrap()
            > 7
    );
}

#[test]
fn module_route_surfaces_dirty_update_as_stale() {
    let project = TempDir::new().unwrap();
    if db::db_exists(project.path()) {
        db::delete_db(project.path()).unwrap();
    }
    let conn = db::open_db(project.path()).unwrap();
    db::init_db(&conn).unwrap();
    conn.execute("INSERT INTO modules (name, path) VALUES ('app', 'app')", [])
        .unwrap();
    conn.execute("INSERT INTO modules (name, path) VALUES ('lib', 'lib')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO module_deps (module_id, dep_module_id, dep_kind)
         SELECT source.id, target.id, 'implementation'
         FROM modules source, modules target
         WHERE source.name = 'app' AND target.name = 'lib'",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO metadata (key, value) VALUES
            ('last_modules_indexed_at', '11'),
            ('last_update_at', '7'),
            ('index_update_dirty_at', '13');",
    )
    .unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(project.path())
        .env(
            "AST_INDEX_DB_PATH",
            db::get_db_path(project.path()).unwrap(),
        )
        .args([
            "--format",
            "json",
            "module-route",
            "--from",
            "app",
            "--to",
            "lib",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "module-route failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning == "index_may_be_stale"),
        "dirty update must reach the module-route staleness boundary: {result}"
    );
}
