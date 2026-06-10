//! Integration tests for `commands::management` public APIs.
//!
//! Targets the SQL-injection guard in `cmd_query` (mutations must be
//! rejected) and the add/remove/list-roots round-trip via the public DB
//! helpers — these were not covered by any integration test.

use std::fs;

use ast_index::commands::management::{
    cmd_add_root, cmd_clear, cmd_db_path, cmd_list_roots, cmd_query, cmd_rebuild, cmd_remove_root,
};
use ast_index::db;
use tempfile::TempDir;

fn open_fresh_db(project_root: &std::path::Path) -> rusqlite::Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn db_has_file(conn: &rusqlite::Connection, path: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
        [path],
        |row| row.get::<_, bool>(0),
    )
    .unwrap()
}

fn db_has_module(conn: &rusqlite::Connection, name: &str, path: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM modules WHERE name = ?1 AND path = ?2)",
        rusqlite::params![name, path],
        |row| row.get::<_, bool>(0),
    )
    .unwrap()
}

// ----------------------------------------------------------------------
// cmd_query — SQL safety
// ----------------------------------------------------------------------

#[test]
fn cmd_query_allows_select() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    db::upsert_file(&conn, "src/Foo.kt", 0, 100).unwrap();
    drop(conn);

    cmd_query(dir.path(), "SELECT path, size FROM files", 10)
        .expect("plain SELECT must be allowed");
}

#[test]
fn cmd_query_allows_with_cte() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());

    cmd_query(dir.path(), "WITH x AS (SELECT 1 AS n) SELECT n FROM x", 10)
        .expect("WITH/CTE queries must be allowed");
}

#[test]
fn cmd_query_allows_explain() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());

    cmd_query(dir.path(), "EXPLAIN SELECT * FROM files", 10)
        .expect("EXPLAIN must be allowed for diagnostics");
}

#[test]
fn cmd_query_rejects_insert() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());

    let err = cmd_query(
        dir.path(),
        "INSERT INTO files (path, mtime, size) VALUES ('x', 0, 0)",
        10,
    )
    .expect_err("INSERT must be rejected by the safety guard");
    assert!(
        err.to_string().contains("SELECT") || err.to_string().to_uppercase().contains("ALLOWED"),
        "rejection message must mention SELECT or 'allowed': {}",
        err
    );
}

#[test]
fn cmd_query_rejects_update() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());
    let err = cmd_query(dir.path(), "UPDATE files SET size = 0", 10)
        .expect_err("UPDATE must be rejected");
    assert!(err.to_string().to_uppercase().contains("ALLOWED"));
}

#[test]
fn cmd_query_rejects_delete() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());
    let err = cmd_query(dir.path(), "DELETE FROM files", 10).expect_err("DELETE must be rejected");
    assert!(err.to_string().to_uppercase().contains("ALLOWED"));
}

#[test]
fn cmd_query_rejects_drop_table() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());
    let err = cmd_query(dir.path(), "DROP TABLE files", 10).expect_err("DROP must be rejected");
    assert!(err.to_string().to_uppercase().contains("ALLOWED"));
}

#[test]
fn cmd_query_rejects_pragma() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());
    let err = cmd_query(dir.path(), "PRAGMA journal_mode", 10)
        .expect_err("PRAGMA must be rejected — could change DB state");
    assert!(err.to_string().to_uppercase().contains("ALLOWED"));
}

// ----------------------------------------------------------------------
// cmd_add_root / cmd_remove_root / cmd_list_roots
// ----------------------------------------------------------------------

#[test]
fn add_root_persists_to_db() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let _ = open_fresh_db(primary.path());

    cmd_add_root(primary.path(), &extra.path().to_string_lossy(), false)
        .expect("add_root for unrelated dir must succeed without --force");

    let conn = db::open_db(primary.path()).unwrap();
    let roots = db::get_extra_roots(&conn).unwrap();
    assert!(
        roots
            .iter()
            .any(|r| r.contains(extra.path().to_string_lossy().as_ref())),
        "extra root must be persisted: {:?}",
        roots
    );
}

#[test]
fn add_root_blocks_overlap_without_force() {
    let primary = TempDir::new().unwrap();
    let _ = open_fresh_db(primary.path());

    // child of primary — overlap, must be refused without --force
    let nested = primary.path().join("subdir");
    fs::create_dir_all(&nested).unwrap();

    cmd_add_root(primary.path(), &nested.to_string_lossy(), false)
        .expect("must Ok-print warning, not error");

    let conn = db::open_db(primary.path()).unwrap();
    let roots = db::get_extra_roots(&conn).unwrap();
    assert!(
        roots.is_empty(),
        "overlap must be blocked without --force, got: {:?}",
        roots
    );
}

#[test]
fn add_root_with_force_bypasses_overlap() {
    let primary = TempDir::new().unwrap();
    let _ = open_fresh_db(primary.path());

    let nested = primary.path().join("nested");
    fs::create_dir_all(&nested).unwrap();

    cmd_add_root(primary.path(), &nested.to_string_lossy(), true)
        .expect("--force must allow overlap");

    let conn = db::open_db(primary.path()).unwrap();
    let roots = db::get_extra_roots(&conn).unwrap();
    assert_eq!(roots.len(), 1);
}

#[test]
fn remove_root_round_trips() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let conn = open_fresh_db(primary.path());

    let extra_path = extra.path().to_string_lossy().to_string();
    db::add_extra_root(&conn, &extra_path).unwrap();
    drop(conn);

    cmd_remove_root(primary.path(), &extra_path).expect("remove_root must succeed");

    let conn = db::open_db(primary.path()).unwrap();
    let roots = db::get_extra_roots(&conn).unwrap();
    assert!(roots.is_empty(), "root must be gone after remove");
}

#[test]
fn list_roots_works_without_extras() {
    let primary = TempDir::new().unwrap();
    let _ = open_fresh_db(primary.path());

    cmd_list_roots(primary.path(), "text").expect("list_roots with no extras must succeed");
}

#[test]
fn add_root_short_circuits_without_db() {
    // No DB created for this primary root.
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();

    cmd_add_root(primary.path(), &extra.path().to_string_lossy(), false)
        .expect("missing DB must print a hint, not error");
}

// ----------------------------------------------------------------------
// cmd_clear / cmd_db_path
// ----------------------------------------------------------------------

#[test]
fn cmd_clear_deletes_database() {
    let dir = TempDir::new().unwrap();
    let _ = open_fresh_db(dir.path());
    assert!(db::db_exists(dir.path()), "DB must exist before clear");

    cmd_clear(dir.path()).expect("clear must succeed");

    assert!(
        !db::db_exists(dir.path()),
        "DB must be gone after cmd_clear"
    );
}

#[test]
fn cmd_db_path_runs_without_db() {
    // db_path is a pure-print helper; must not require a DB to exist.
    let dir = TempDir::new().unwrap();
    cmd_db_path(dir.path()).expect("db_path must succeed even without DB");
}

// ----------------------------------------------------------------------
// cmd_rebuild
// ----------------------------------------------------------------------

#[test]
fn rebuild_sub_projects_keeps_root_direct_entries() {
    let repo = TempDir::new().unwrap();
    fs::create_dir(repo.path().join(".arc")).unwrap();
    fs::write(repo.path().join(".arc").join("HEAD"), "trunk\n").unwrap();

    let root = repo.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("ya.make"), "LIBRARY()\nEND()\n").unwrap();
    fs::write(
        root.join("root.sh"),
        "ROOT_VAR=1\nroot_func() {\n  echo ok\n}\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("app")).unwrap();
    fs::write(root.join("app").join("ya.make"), "PROGRAM()\nEND()\n").unwrap();
    fs::write(
        root.join("app").join("main.sh"),
        "APP_VAR=1\napp_func() {\n  echo ok\n}\n",
    )
    .unwrap();

    cmd_rebuild(&root, "all", false, false, true, false, true, &[], &[], &[])
        .expect("sub-project rebuild must succeed");

    let conn = db::open_db(&root).unwrap();
    assert!(db_has_file(&conn, "root.sh"));
    assert!(db_has_file(&conn, "app/main.sh"));
    assert!(db_has_module(&conn, "workspace", ""));
    assert!(db_has_module(&conn, "workspace/app", "app"));
}

/// Regression: `cmd_rebuild` used to round-trip subtrees through the legacy
/// `metadata.extra_roots` JSON column (just canonical paths), wiping the
/// user-chosen name and the original (relative) path form on every rebuild.
/// After the fix, both fields survive verbatim.
#[test]
fn rebuild_preserves_subtree_names_and_original_paths() {
    let primary = TempDir::new().unwrap();
    let extra1 = TempDir::new().unwrap();
    let extra2 = TempDir::new().unwrap();

    fs::write(primary.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(extra1.path().join("a.rs"), "fn a() {}\n").unwrap();
    fs::write(extra2.path().join("b.rs"), "fn b() {}\n").unwrap();

    // Initial rebuild + manually attach two named subtrees with distinct
    // original_path forms (one absolute, one relative-style).
    cmd_rebuild(
        primary.path(),
        "all",
        false,
        false,
        false,
        false,
        true,
        &[],
        &[],
        &[],
    )
    .expect("initial rebuild must succeed");

    {
        let conn = db::open_db(primary.path()).unwrap();
        let extra1_canonical = db::safe_canonicalize(extra1.path())
            .to_string_lossy()
            .into_owned();
        let extra2_canonical = db::safe_canonicalize(extra2.path())
            .to_string_lossy()
            .into_owned();
        db::insert_subtree(
            &conn,
            "my-extra1",
            &extra1_canonical,
            &extra1.path().to_string_lossy(),
        )
        .unwrap();
        db::insert_subtree(
            &conn,
            "my-extra2",
            &extra2_canonical,
            "../extra2-relative",
        )
        .unwrap();
    }

    // Rebuild again. Pre-fix, this collapsed `(name, canonical, original)`
    // to bare canonical-only and re-imported as `(basename, canonical,
    // canonical)`.
    cmd_rebuild(
        primary.path(),
        "all",
        false,
        false,
        false,
        false,
        true,
        &[],
        &[],
        &[],
    )
    .expect("second rebuild must succeed");

    let conn = db::open_db(primary.path()).unwrap();
    let subtrees = db::list_subtrees(&conn).unwrap();
    assert_eq!(
        subtrees.len(),
        2,
        "both subtrees must survive rebuild, got: {:?}",
        subtrees
    );

    let s1 = subtrees
        .iter()
        .find(|s| s.name == "my-extra1")
        .expect("user-chosen name 'my-extra1' must survive rebuild");
    assert_eq!(
        s1.original_path,
        extra1.path().to_string_lossy(),
        "original_path must survive verbatim, not be replaced by canonical"
    );

    let s2 = subtrees
        .iter()
        .find(|s| s.name == "my-extra2")
        .expect("user-chosen name 'my-extra2' must survive rebuild");
    assert_eq!(
        s2.original_path, "../extra2-relative",
        "relative original_path must survive verbatim across rebuild"
    );
}
