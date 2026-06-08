//! Step 1 of #31: schema + DB helpers for named workspace subtrees.
//! Make sure the new `subtrees` table works on its own AND that the
//! legacy `metadata.extra_roots` JSON is silently migrated into it on
//! the first read after upgrade.

use std::path::Path;

use ast_index::db;
use rusqlite::{params, Connection};
use tempfile::TempDir;

fn open_fresh(project_root: &Path) -> Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

#[test]
fn empty_project_has_no_subtrees() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    assert!(db::list_subtrees(&conn).unwrap().is_empty());
    assert!(db::get_extra_roots(&conn).unwrap().is_empty());
}

#[test]
fn insert_subtree_then_query() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());

    db::insert_subtree(&conn, "grut", "/Users/u/p/grut", "../grut").unwrap();
    db::insert_subtree(&conn, "adv", "/Users/u/p/adv", "../adv/frontend").unwrap();

    let all = db::list_subtrees(&conn).unwrap();
    assert_eq!(all.len(), 2);
    // ORDER BY name → adv first, grut second.
    assert_eq!(all[0].name, "adv");
    assert_eq!(all[0].canonical_path, "/Users/u/p/adv");
    assert_eq!(all[0].original_path, "../adv/frontend");
    assert_eq!(all[1].name, "grut");

    let by_name = db::find_subtree_by_name(&conn, "grut").unwrap().unwrap();
    assert_eq!(by_name.original_path, "../grut");

    let missing = db::find_subtree_by_name(&conn, "ghost").unwrap();
    assert!(missing.is_none());

    let by_path = db::find_subtree_by_root_path(&conn, "/Users/u/p/grut")
        .unwrap()
        .unwrap();
    assert_eq!(by_path.name, "grut");
}

#[test]
fn duplicate_name_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    db::insert_subtree(&conn, "grut", "/a", "/a").unwrap();
    let err = db::insert_subtree(&conn, "grut", "/b", "/b");
    assert!(err.is_err());
}

#[test]
fn duplicate_canonical_path_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    db::insert_subtree(&conn, "grut1", "/shared", "/shared").unwrap();
    let err = db::insert_subtree(&conn, "grut2", "/shared", "/shared");
    assert!(err.is_err());
}

#[test]
fn remove_subtree_returns_true_when_found() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    db::insert_subtree(&conn, "grut", "/a", "/a").unwrap();
    assert!(db::remove_subtree_by_name(&conn, "grut").unwrap());
    assert!(!db::remove_subtree_by_name(&conn, "grut").unwrap());
    assert!(db::list_subtrees(&conn).unwrap().is_empty());
}

#[test]
fn default_name_derivation_is_friendly() {
    assert_eq!(db::default_subtree_name("/Users/u/p/grut"), "grut");
    assert_eq!(db::default_subtree_name("/Users/u/p/grut/"), "grut");
    assert_eq!(db::default_subtree_name("../adv/frontend"), "frontend");
    assert_eq!(db::default_subtree_name("../"), "subtree");
    assert_eq!(db::default_subtree_name("/"), "subtree");
    // Non-alphanumeric chars become dashes, trimmed.
    let messy = db::default_subtree_name("/path/with spaces & punctuation!");
    assert!(!messy.is_empty());
    assert!(!messy.starts_with('-'));
    assert!(!messy.ends_with('-'));
}

#[test]
fn allocate_name_handles_collisions() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    assert_eq!(
        db::allocate_subtree_name(&conn, "grut").unwrap(),
        "grut",
        "first allocation gets preferred name"
    );
    db::insert_subtree(&conn, "grut", "/a", "/a").unwrap();
    assert_eq!(
        db::allocate_subtree_name(&conn, "grut").unwrap(),
        "grut-2",
        "second allocation appends -2"
    );
    db::insert_subtree(&conn, "grut-2", "/b", "/b").unwrap();
    assert_eq!(
        db::allocate_subtree_name(&conn, "grut").unwrap(),
        "grut-3"
    );
}

#[test]
fn legacy_extra_roots_migrate_on_first_read() {
    // Build a fresh DB and stuff a pre-3.47 JSON row into metadata to
    // simulate an upgrade from an older ast-index version.
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    let json = serde_json::to_string(&vec![
        "/Users/u/p/grut".to_string(),
        "/Users/u/p/adv".to_string(),
    ])
    .unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('extra_roots', ?1)",
        params![json],
    )
    .unwrap();

    // First call triggers the migration.
    let roots = db::get_extra_roots(&conn).unwrap();
    assert_eq!(roots.len(), 2);
    assert!(roots.iter().any(|r| r == "/Users/u/p/grut"));
    assert!(roots.iter().any(|r| r == "/Users/u/p/adv"));

    let subtrees = db::list_subtrees(&conn).unwrap();
    assert_eq!(subtrees.len(), 2);
    // Names default to the basename of the path.
    let names: Vec<&str> = subtrees.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"grut"));
    assert!(names.contains(&"adv"));

    // The legacy metadata row is gone after migration.
    let still_there: rusqlite::Result<String> = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'extra_roots'",
        [],
        |row| row.get(0),
    );
    assert!(matches!(
        still_there,
        Err(rusqlite::Error::QueryReturnedNoRows)
    ));

    // Second call is a noop (no duplicates, no errors).
    let roots2 = db::get_extra_roots(&conn).unwrap();
    assert_eq!(roots2.len(), 2);
}

#[test]
fn add_extra_root_compat_shim_uses_subtrees_table() {
    let tmp = TempDir::new().unwrap();
    let conn = open_fresh(tmp.path());
    db::add_extra_root(&conn, "/Users/u/p/grut").unwrap();
    db::add_extra_root(&conn, "/Users/u/p/grut").unwrap(); // dedup
    db::add_extra_root(&conn, "/Users/u/p/adv").unwrap();

    let subtrees = db::list_subtrees(&conn).unwrap();
    assert_eq!(subtrees.len(), 2);
    let names: Vec<&str> = subtrees.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"grut"));
    assert!(names.contains(&"adv"));

    // Compat: remove via canonical path also flows through subtrees.
    assert!(db::remove_extra_root(&conn, "/Users/u/p/grut").unwrap());
    let after = db::list_subtrees(&conn).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "adv");
}
