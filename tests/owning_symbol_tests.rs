//! `db::find_owning_symbol` — which symbol's body contains a given line.
//!
//! The interesting cases are all about `symbols.end_line` being nullable:
//! a language with ranges must get exact containment, a language without
//! them must keep the pre-`end_line` behaviour, and a file that mixes the
//! two must not let a range-less one-liner swallow the rest of the file.

use std::fs;
use std::path::Path;

use ast_index::{db, indexer};
use rusqlite::{params, Connection};
use tempfile::TempDir;

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn add_file(conn: &Connection, path: &str) -> i64 {
    conn.execute(
        "INSERT INTO files (path, root_path, mtime, size) VALUES (?1, '', 0, 0)",
        params![path],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn add_symbol(conn: &Connection, file_id: i64, name: &str, line: i64, end_line: Option<i64>) {
    conn.execute(
        "INSERT INTO symbols (file_id, name, kind, line, end_line) VALUES (?1, ?2, 'function', ?3, ?4)",
        params![file_id, name, line, end_line],
    )
    .unwrap();
}

fn owner_of(conn: &Connection, path: &str, line: i64) -> Option<String> {
    db::find_owning_symbol(conn, None, path, line)
        .unwrap()
        .map(|symbol| symbol.name)
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn nested_ranges_resolve_to_the_narrowest_one() {
    let conn = fresh_db();
    let file = add_file(&conn, "app/greeter.rb");
    add_symbol(&conn, file, "Greeter", 1, Some(20));
    add_symbol(&conn, file, "hello", 3, Some(8));
    add_symbol(&conn, file, "bye", 10, Some(15));

    assert_eq!(
        owner_of(&conn, "app/greeter.rb", 5).as_deref(),
        Some("hello")
    );
    assert_eq!(
        owner_of(&conn, "app/greeter.rb", 12).as_deref(),
        Some("bye")
    );
    // Between the two methods, only the class still contains the line.
    assert_eq!(
        owner_of(&conn, "app/greeter.rb", 9).as_deref(),
        Some("Greeter")
    );
}

#[test]
fn a_line_past_every_range_has_no_owner() {
    let conn = fresh_db();
    let file = add_file(&conn, "app/greeter.rb");
    add_symbol(&conn, file, "Greeter", 1, Some(20));
    add_symbol(&conn, file, "bye", 10, Some(15));

    // This is the module-level case: the legacy heuristic answered "bye".
    assert_eq!(owner_of(&conn, "app/greeter.rb", 25), None);
}

#[test]
fn a_range_less_symbol_owns_only_its_own_line() {
    let conn = fresh_db();
    let file = add_file(&conn, "app/greeter.rb");
    add_symbol(&conn, file, "Greeter", 1, Some(40));
    add_symbol(&conn, file, "MAX_RETRIES", 4, None);
    add_symbol(&conn, file, "bye", 10, Some(15));

    assert_eq!(
        owner_of(&conn, "app/greeter.rb", 4).as_deref(),
        Some("MAX_RETRIES")
    );
    // Line 6 is past the one-liner, so the class takes it back.
    assert_eq!(
        owner_of(&conn, "app/greeter.rb", 6).as_deref(),
        Some("Greeter")
    );
    assert_eq!(
        owner_of(&conn, "app/greeter.rb", 12).as_deref(),
        Some("bye")
    );
}

#[test]
fn a_file_without_any_range_falls_back_to_the_last_declaration() {
    let conn = fresh_db();
    let file = add_file(&conn, "styles/app.css");
    add_symbol(&conn, file, "header", 3, None);
    add_symbol(&conn, file, "footer", 30, None);

    // No range data at all: keep answering as before `end_line` existed.
    assert_eq!(
        owner_of(&conn, "styles/app.css", 12).as_deref(),
        Some("header")
    );
    assert_eq!(
        owner_of(&conn, "styles/app.css", 300).as_deref(),
        Some("footer")
    );
    // Above every declaration there is still nothing to attribute to.
    assert_eq!(owner_of(&conn, "styles/app.css", 1), None);
}

#[test]
fn the_fallback_does_not_leak_into_files_that_have_ranges() {
    let conn = fresh_db();
    let ranged = add_file(&conn, "app/greeter.rb");
    add_symbol(&conn, ranged, "hello", 3, Some(8));
    let flat = add_file(&conn, "styles/app.css");
    add_symbol(&conn, flat, "header", 3, None);

    assert_eq!(owner_of(&conn, "app/greeter.rb", 50), None);
    assert_eq!(
        owner_of(&conn, "styles/app.css", 50).as_deref(),
        Some("header")
    );
}

#[test]
fn an_unknown_file_has_no_owner() {
    let conn = fresh_db();
    add_file(&conn, "app/greeter.rb");
    assert_eq!(owner_of(&conn, "app/missing.rb", 5), None);
}

#[test]
fn file_has_symbol_ranges_reports_per_file_support() {
    let conn = fresh_db();
    let ranged = add_file(&conn, "app/greeter.rb");
    add_symbol(&conn, ranged, "hello", 3, Some(8));
    let flat = add_file(&conn, "styles/app.css");
    add_symbol(&conn, flat, "header", 3, None);

    assert!(db::file_has_symbol_ranges(&conn, None, "app/greeter.rb").unwrap());
    assert!(!db::file_has_symbol_ranges(&conn, None, "styles/app.css").unwrap());
    assert!(!db::file_has_symbol_ranges(&conn, None, "app/missing.rb").unwrap());
}

#[test]
fn indexed_ruby_attributes_a_call_to_its_method_not_the_class() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("app/greeter.rb"),
        "class Greeter\n  def hello\n    render_name\n  end\n\n  def bye\n    'bye'\n  end\nend\n",
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();
    let root_key = db::normalize_root_for_storage(project.path());

    let owner = db::find_owning_symbol(&conn, Some(&root_key), "app/greeter.rb", 3)
        .unwrap()
        .expect("line 3 is inside Greeter#hello");
    assert_eq!(owner.name, "hello");

    // The `end` of the class is inside the class but outside both methods.
    let owner = db::find_owning_symbol(&conn, Some(&root_key), "app/greeter.rb", 5)
        .unwrap()
        .expect("line 5 is still inside the class body");
    assert_eq!(owner.name, "Greeter");
}

#[test]
fn a_fresh_database_carries_the_owner_lookup_index() {
    let conn = fresh_db();
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'idx_symbols_file_line_end'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        sql.contains("symbols(file_id, line, end_line)"),
        "unexpected index definition: {sql}"
    );
}

fn add_file_under(conn: &Connection, root_path: &str, path: &str) -> i64 {
    conn.execute(
        "INSERT INTO files (path, root_path, mtime, size) VALUES (?1, ?2, 0, 0)",
        params![path, root_path],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn a_path_shared_by_two_roots_resolves_within_the_root_asked_for() {
    let conn = fresh_db();
    let primary = add_file(&conn, "app/greeter.rb");
    add_symbol(&conn, primary, "Greeter", 1, Some(20));
    add_symbol(&conn, primary, "hello", 3, Some(8));
    // Narrower than `hello` on the same lines, but in another root.
    let shared = add_file_under(&conn, "/work/shared", "app/greeter.rb");
    add_symbol(&conn, shared, "shadow", 4, Some(5));
    let flat = add_file_under(&conn, "/work/flat", "app/greeter.rb");
    add_symbol(&conn, flat, "header", 2, None);

    let owner = |root: Option<&str>, line| {
        db::find_owning_symbol(&conn, root, "app/greeter.rb", line)
            .unwrap()
            .map(|symbol| symbol.name)
    };
    assert_eq!(owner(None, 4).as_deref(), Some("hello"));
    assert_eq!(owner(Some("/work/shared"), 4).as_deref(), Some("shadow"));
    assert_eq!(owner(Some("/work/shared"), 7), None);
    // The last-declaration fallback applies to the range-less file alone.
    assert_eq!(owner(Some("/work/flat"), 7).as_deref(), Some("header"));
    assert_eq!(owner(None, 30), None);
    assert_eq!(owner(Some("/work/elsewhere"), 4), None);

    let names = |root: Option<&str>| {
        db::get_file_symbols(&conn, root, "app/greeter.rb")
            .unwrap()
            .into_iter()
            .map(|symbol| symbol.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(None), ["Greeter", "hello"]);
    assert_eq!(names(Some("/work/shared")), ["shadow"]);

    assert!(db::file_has_symbol_ranges(&conn, None, "app/greeter.rb").unwrap());
    assert!(!db::file_has_symbol_ranges(&conn, Some("/work/flat"), "app/greeter.rb").unwrap());
}

#[test]
fn the_primary_root_is_found_under_either_spelling() {
    let conn = fresh_db();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('project_root', '/work/app')",
        [],
    )
    .unwrap();
    // Indexes created before `root_path` keep the primary root as ''.
    let legacy = add_file(&conn, "app/legacy.rb");
    add_symbol(&conn, legacy, "legacy", 1, Some(5));
    let current = add_file_under(&conn, "/work/app", "app/current.rb");
    add_symbol(&conn, current, "current", 1, Some(5));
    let shared = add_file_under(&conn, "/work/shared", "app/shared.rb");
    add_symbol(&conn, shared, "shared", 1, Some(5));

    let owner = |root: Option<&str>, path| {
        db::find_owning_symbol(&conn, root, path, 2)
            .unwrap()
            .map(|symbol| symbol.name)
    };
    for root in [None, Some(""), Some("/work/app")] {
        assert_eq!(owner(root, "app/legacy.rb").as_deref(), Some("legacy"));
        assert_eq!(owner(root, "app/current.rb").as_deref(), Some("current"));
        assert_eq!(owner(root, "app/shared.rb"), None);
    }
    assert_eq!(owner(Some("/work/shared"), "app/legacy.rb"), None);
}

#[test]
fn a_fresh_database_leaves_out_the_prefix_of_the_owner_lookup_index() {
    let conn = fresh_db();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'idx_symbols_file')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!exists);
    // Per-file lookups and the cascade from `files` seek on its prefix.
    let plan: Vec<String> = conn
        .prepare("EXPLAIN QUERY PLAN SELECT name FROM symbols WHERE file_id = ?1")
        .unwrap()
        .query_map([1], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|step| step.contains("idx_symbols_file_line_end (file_id=?)")),
        "{plan:?}"
    );
}
