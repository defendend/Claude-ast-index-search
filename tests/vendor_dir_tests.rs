//! Regression tests for issue #47: a project-owned `vendor/` directory must
//! not be silently skipped by ast-index's internal ignore list.

use std::fs;
use std::path::Path;

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn open_fresh_db(project_root: &Path) -> Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn vendor_directory_is_indexed_like_project_source() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    write(
        &root.join("Cargo.toml"),
        "[package]\nname=\"vendor-regression\"\nversion=\"0.0.0\"\n",
    );
    write(
        &root.join("vendor/lib.rs"),
        "pub fn vendored_project_symbol() {}\n",
    );

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    let files = db::find_files(&conn, "vendor/lib.rs", 10).unwrap();
    assert_eq!(files, vec!["vendor/lib.rs".to_string()]);

    let symbols = db::find_symbols_by_name(&conn, "vendored_project_symbol", None, 10).unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].path, "vendor/lib.rs");
}
