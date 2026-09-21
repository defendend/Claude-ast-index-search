//! `open_db` must add `symbols.end_line` to an index built before the column
//! existed, without a forced rebuild and without losing existing rows.
//!
//! This target intentionally contains one test: `AST_INDEX_DB_PATH` is a
//! process-wide override, so it must not race other scenarios.

use std::ffi::OsString;
use std::path::Path;

use ast_index::db;
use rusqlite::{params, Connection};
use tempfile::TempDir;

struct DbPathOverride {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl DbPathOverride {
    fn new(path: &Path) -> Self {
        let keys = [
            "AST_INDEX_DB_PATH",
            "AST_INDEX_NO_CANONICALIZE",
            "AST_INDEX_CANONICALIZE_TIMEOUT_MS",
        ];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("AST_INDEX_DB_PATH", path);
        std::env::remove_var("AST_INDEX_NO_CANONICALIZE");
        std::env::remove_var("AST_INDEX_CANONICALIZE_TIMEOUT_MS");
        Self { previous }
    }
}

impl Drop for DbPathOverride {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// A schema as shipped before `end_line` existed: `qualified_name` is already
/// there, so the only missing piece is the new column.
fn create_db_without_end_line(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            root_path TEXT NOT NULL DEFAULT '',
            mtime INTEGER NOT NULL,
            size INTEGER NOT NULL,
            UNIQUE(root_path, path)
        );

        CREATE TABLE symbols (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            qualified_name TEXT,
            kind TEXT NOT NULL,
            line INTEGER NOT NULL,
            parent_id INTEGER,
            signature TEXT,
            FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
        );

        CREATE TABLE metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE subtrees (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            canonical_path TEXT NOT NULL UNIQUE,
            original_path TEXT NOT NULL
        );

        INSERT INTO files (id, path, root_path, mtime, size)
        VALUES (1, 'app/greeter.rb', '', 123, 456);
        INSERT INTO symbols (id, file_id, name, kind, line, signature)
        VALUES (1, 1, 'Greeter', 'class', 7, 'class Greeter');
        "#,
    )
    .unwrap();
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, column],
        |row| row.get(0),
    )
    .unwrap()
}

fn index_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
        params![name],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn open_db_adds_end_line_column_to_an_existing_index() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let db_file = tmp.path().join("legacy.sqlite");
    create_db_without_end_line(&db_file);

    let _override = DbPathOverride::new(&db_file);

    let conn = db::open_db(&project).unwrap();
    assert!(column_exists(&conn, "symbols", "end_line"));
    // The owner-lookup index covers the new column, so the open that adds the
    // column has to install it too — not the one after it.
    assert!(index_exists(&conn, "idx_symbols_file_line_end"));

    // The pre-existing row survives and reads back as "range unknown".
    let (name, line, end_line): (String, i64, Option<i64>) = conn
        .query_row(
            "SELECT name, line, end_line FROM symbols WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((name.as_str(), line, end_line), ("Greeter", 7, None));

    // The migrated column accepts writes from the new indexer path.
    conn.execute(
        "INSERT INTO symbols (file_id, name, kind, line, end_line, signature)
         VALUES (1, 'hello', 'function', 8, 10, 'def hello')",
        [],
    )
    .unwrap();
    let stored: Option<i64> = conn
        .query_row(
            "SELECT end_line FROM symbols WHERE name = 'hello'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, Some(10));
    drop(conn);

    // Re-opening an already-migrated DB is a no-op, not an error.
    let conn = db::open_db(&project).unwrap();
    assert!(column_exists(&conn, "symbols", "end_line"));
    let symbol_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .unwrap();
    assert_eq!(symbol_count, 2);
}
