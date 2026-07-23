//! Regression tests for `PathResolver`.
//!
//! Bug: search output printed stored relative paths without any hint of
//! which root they belonged to. With extra roots (e.g. `add-root /other`)
//! a file at `src/.../BClass.java` is ambiguous — an agent would look under
//! the primary project and miss the real file in the extra root.
//!
//! The fix: when extra roots are configured, resolve each stored relative
//! path to an absolute path by probing roots on disk.

use std::fs;

use ast_index::commands::PathResolver;
use ast_index::db;
use rusqlite::{params, Connection};
use tempfile::TempDir;

fn open_fresh_db(_project_root: &std::path::Path) -> rusqlite::Connection {
    // PathResolver only needs the schema. Keeping these connections in-memory
    // avoids making parallel tests contend through the process-wide cache
    // namespace now that unreadable cache candidates deliberately fail closed.
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    conn
}

#[test]
fn resolve_is_identity_without_extra_roots() {
    let primary = TempDir::new().unwrap();
    let conn = open_fresh_db(primary.path());

    let resolver = PathResolver::from_conn(primary.path(), &conn);
    assert_eq!(
        resolver.resolve("src/Foo.kt"),
        "src/Foo.kt",
        "single-root output must stay byte-for-byte identical"
    );
}

#[test]
fn resolve_returns_absolute_path_in_extra_root() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let conn = open_fresh_db(primary.path());

    db::add_extra_root(&conn, &extra.path().to_string_lossy()).unwrap();

    let rel = "src/main/java/some/deep/located/BClass.java";
    let file = extra.path().join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "class BClass {}").unwrap();

    let resolver = PathResolver::from_conn(primary.path(), &conn);
    let resolved = resolver.resolve(rel);

    assert_eq!(
        resolved,
        std::path::Path::new(&db::normalize_root_for_storage(extra.path()))
            .join(rel)
            .to_string_lossy(),
        "file present only under extra root must resolve to its absolute path"
    );
}

#[test]
fn strict_resolver_migrates_legacy_direct_connection() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .unwrap();
    let raw_extra = extra.path().to_string_lossy().into_owned();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('extra_roots', ?1)",
        params![serde_json::to_string(&vec![raw_extra]).unwrap()],
    )
    .unwrap();

    let rel = "src/Legacy.kt";
    fs::create_dir_all(extra.path().join("src")).unwrap();
    fs::write(extra.path().join(rel), "class Legacy").unwrap();

    let resolver = PathResolver::try_from_conn(primary.path(), &conn).unwrap();
    assert_eq!(
        resolver.resolve(rel),
        std::path::Path::new(&db::normalize_root_for_storage(extra.path()))
            .join(rel)
            .to_string_lossy()
    );
    assert_eq!(db::list_subtrees(&conn).unwrap().len(), 1);
}

#[test]
fn resolve_prefers_primary_when_both_roots_have_file() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let conn = open_fresh_db(primary.path());

    db::add_extra_root(&conn, &extra.path().to_string_lossy()).unwrap();

    let rel = "src/Foo.kt";
    fs::create_dir_all(primary.path().join("src")).unwrap();
    fs::create_dir_all(extra.path().join("src")).unwrap();
    fs::write(primary.path().join(rel), "primary").unwrap();
    fs::write(extra.path().join(rel), "extra").unwrap();

    let resolver = PathResolver::from_conn(primary.path(), &conn);
    assert_eq!(
        resolver.resolve(rel),
        primary.path().join(rel).to_string_lossy(),
        "primary root must win on path collision — agent gets local version"
    );
}

#[test]
fn resolve_falls_back_to_relative_when_nothing_exists() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let conn = open_fresh_db(primary.path());

    db::add_extra_root(&conn, &extra.path().to_string_lossy()).unwrap();

    let resolver = PathResolver::from_conn(primary.path(), &conn);
    assert_eq!(
        resolver.resolve("ghost/Missing.kt"),
        "ghost/Missing.kt",
        "stale index entries must fall back to rel path, not lie about location"
    );
}

#[test]
fn resolve_with_root_disambiguates_colliding_paths() {
    let primary = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let conn = open_fresh_db(primary.path());

    db::add_extra_root(&conn, &extra.path().to_string_lossy()).unwrap();

    let rel = "src/Foo.kt";
    fs::create_dir_all(primary.path().join("src")).unwrap();
    fs::create_dir_all(extra.path().join("src")).unwrap();
    fs::write(primary.path().join(rel), "primary").unwrap();
    fs::write(extra.path().join(rel), "extra").unwrap();

    let resolver = PathResolver::from_conn(primary.path(), &conn).with_decoration(false);
    let resolved =
        resolver.resolve_with_root(rel, Some(&db::normalize_root_for_storage(extra.path())));

    assert_eq!(
        resolved,
        std::path::Path::new(&db::normalize_root_for_storage(extra.path()))
            .join(rel)
            .to_string_lossy(),
        "owning root hint must point to the extra-root copy, not the primary one"
    );
}
