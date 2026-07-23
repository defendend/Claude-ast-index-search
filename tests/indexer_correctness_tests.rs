use std::fs;
use std::path::Path;
use std::time::SystemTime;

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

#[test]
fn incremental_update_detects_size_change_at_same_mtime() {
    let project = TempDir::new().unwrap();
    let source = project.path().join("src/lib.rs");
    write_file(&source, "pub struct Before;\n");

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    write_file(&source, "pub struct AfterWithLongerName;\n");
    let metadata = fs::metadata(&source).unwrap();
    let current_mtime = metadata
        .modified()
        .unwrap()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let current_size = metadata.len() as i64;

    let stored_size: i64 = conn
        .query_row(
            "SELECT size FROM files WHERE path = 'src/lib.rs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(stored_size, current_size, "fixture must change file size");

    // Simulate a coarse timestamp filesystem: content changed, but its
    // second-resolution mtime is identical to the stored value.
    conn.execute(
        "UPDATE files SET mtime = ?1 WHERE path = 'src/lib.rs'",
        [current_mtime],
    )
    .unwrap();

    let (updated, changed, deleted) =
        indexer::update_directory_incremental(&mut conn, project.path(), false, None, None)
            .unwrap();

    assert_eq!((updated, changed, deleted), (1, 1, 0));
    let symbols: Vec<String> = conn
        .prepare("SELECT name FROM symbols ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(symbols.iter().any(|name| name == "AfterWithLongerName"));
    assert!(!symbols.iter().any(|name| name == "Before"));
}

#[test]
fn successful_incremental_update_records_completion_timestamp() {
    let project = TempDir::new().unwrap();
    let source = project.path().join("src/lib.rs");
    write_file(&source, "pub struct Before;\n");

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_update_at', '7')",
        [],
    )
    .unwrap();

    write_file(&source, "pub struct AfterWithLongerName;\n");
    let (updated, _, _) =
        indexer::update_directory_incremental(&mut conn, project.path(), false, None, None)
            .unwrap();

    assert_eq!(updated, 1);
    let recorded = metadata_value(&conn, "last_update_at")
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(recorded > 7);
}

#[test]
fn failed_incremental_update_does_not_advance_completion_timestamp() {
    let project = TempDir::new().unwrap();
    let source = project.path().join("src/lib.rs");
    write_file(&source, "pub struct Before;\n");

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_update_at', '7')",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_file_update BEFORE INSERT ON files
         BEGIN SELECT RAISE(ABORT, 'forced file update failure'); END;",
    )
    .unwrap();

    write_file(&source, "pub struct AfterWithLongerName;\n");
    let error = indexer::update_directory_incremental(&mut conn, project.path(), false, None, None)
        .unwrap_err();

    assert!(format!("{error:#}").contains("forced file update failure"));
    assert_eq!(
        metadata_value(&conn, "last_update_at").as_deref(),
        Some("7")
    );
}

#[test]
fn module_dependency_batch_deduplicates_identical_edges() {
    let project = TempDir::new().unwrap();
    let target = project.path().join("core/network/build.gradle.kts");
    let consumer = project.path().join("feature/login/build.gradle.kts");
    write_file(&target, "");
    write_file(
        &consumer,
        "dependencies { implementation(project(\":core:network\")) }\n",
    );

    let mut conn = fresh_db();
    let module_files = vec![target, consumer.clone()];
    indexer::index_modules_from_files(&conn, project.path(), &module_files).unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_modules_indexed_at', '11')",
        [],
    )
    .unwrap();

    // Collected build-file lists can overlap when roots/scopes overlap. The
    // storage batch must still persist one logical edge.
    let duplicated_input = vec![consumer.clone(), consumer];
    let indexed =
        indexer::index_module_dependencies(&mut conn, project.path(), &duplicated_input, false)
            .unwrap();

    let stored: i64 = conn
        .query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))
        .unwrap();
    assert_eq!(indexed, 1);
    assert_eq!(stored, 1);
    let recorded = metadata_value(&conn, "last_modules_indexed_at")
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(recorded > 11);
}

#[test]
fn failed_module_dependency_index_does_not_advance_completion_timestamp() {
    let project = TempDir::new().unwrap();
    let target = project.path().join("core/network/build.gradle.kts");
    let consumer = project.path().join("feature/login/build.gradle.kts");
    write_file(&target, "");
    write_file(
        &consumer,
        "dependencies { implementation(project(\":core:network\")) }\n",
    );

    let mut conn = fresh_db();
    let module_files = vec![target, consumer.clone()];
    indexer::index_modules_from_files(&conn, project.path(), &module_files).unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('last_modules_indexed_at', '11')",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_module_dep BEFORE INSERT ON module_deps
         BEGIN SELECT RAISE(ABORT, 'forced module dependency failure'); END;",
    )
    .unwrap();

    let error = indexer::index_module_dependencies(&mut conn, project.path(), &[consumer], false)
        .unwrap_err();

    assert!(format!("{error:#}").contains("forced module dependency failure"));
    assert_eq!(
        metadata_value(&conn, "last_modules_indexed_at").as_deref(),
        Some("11")
    );
}
