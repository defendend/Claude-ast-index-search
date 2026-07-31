//! Regression coverage for eager, transactional upgrades performed by `open_db`.
//!
//! This target intentionally contains one test: `AST_INDEX_DB_PATH` is a
//! process-wide override, so all scenarios that need a handcrafted on-disk DB
//! run sequentially in the same test process.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use ast_index::{commands::PathResolver, db};
use rusqlite::{params, Connection, OptionalExtension};
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

    fn set(&self, path: &Path) {
        std::env::set_var("AST_INDEX_DB_PATH", path);
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

fn create_legacy_db(path: &Path, extra_roots: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            mtime INTEGER NOT NULL,
            size INTEGER NOT NULL
        );

        CREATE TABLE symbols (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL,
            name TEXT NOT NULL,
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

        INSERT INTO files (id, path, mtime, size)
        VALUES (1, 'src/Legacy.kt', 123, 456);
        INSERT INTO symbols (id, file_id, name, kind, line, signature)
        VALUES (1, 1, 'Legacy', 'class', 7, 'class Legacy');
        INSERT INTO metadata (key, value)
        VALUES ('project_root', 'legacy-project-root');
        "#,
    )
    .unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('extra_roots', ?1)",
        params![extra_roots],
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

fn schema_object_exists(conn: &Connection, kind: &str, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2)",
        params![kind, name],
        |row| row.get(0),
    )
    .unwrap()
}

fn metadata_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM metadata WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

fn unique_index_columns(conn: &Connection, table: &str) -> Vec<Vec<String>> {
    let mut indexes = conn
        .prepare("SELECT name FROM pragma_index_list(?1) WHERE \"unique\" = 1 ORDER BY name")
        .unwrap();
    let names = indexes
        .query_map([table], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut columns = conn
                .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                .unwrap();
            columns
                .query_map([name], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
}

#[test]
fn open_db_migrates_legacy_schemas_atomically_and_idempotently() {
    let tmp = TempDir::new().unwrap();

    let valid_project = tmp.path().join("valid-project");
    std::fs::create_dir(&valid_project).unwrap();
    let valid_db = tmp.path().join("valid.sqlite");
    create_legacy_db(&valid_db, r#"["/legacy/grut","/legacy/adv"]"#);

    let db_path_override = DbPathOverride::new(&valid_db);
    {
        let conn = db::open_db(&valid_project).unwrap();

        let subtrees = db::list_subtrees(&conn).unwrap();
        assert_eq!(subtrees.len(), 2);
        assert_eq!(subtrees[0].name, "adv");
        assert_eq!(subtrees[0].canonical_path, "/legacy/adv");
        assert_eq!(subtrees[0].original_path, "/legacy/adv");
        assert_eq!(subtrees[1].name, "grut");
        assert_eq!(subtrees[1].canonical_path, "/legacy/grut");

        assert!(column_exists(&conn, "files", "root_path"));
        assert!(column_exists(&conn, "symbols", "qualified_name"));
        assert!(schema_object_exists(
            &conn,
            "index",
            "idx_symbols_qualified_name"
        ));

        let root_path: String = conn
            .query_row("SELECT root_path FROM files WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let qualified_name: Option<String> = conn
            .query_row(
                "SELECT qualified_name FROM symbols WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(root_path, "");
        assert_eq!(qualified_name, None);
        assert_eq!(metadata_value(&conn, "extra_roots"), None);

        conn.execute(
            "INSERT INTO files (path, root_path, mtime, size) VALUES ('src/lib.rs', '', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files (path, root_path, mtime, size) VALUES ('src/lib.rs', '/legacy/grut', 2, 2)",
            [],
        )
        .unwrap();
        let colliding_paths: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'src/lib.rs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(colliding_paths, 2);

        let unique_columns = unique_index_columns(&conn, "files");
        assert!(unique_columns.contains(&vec!["root_path".into(), "path".into()]));
        assert!(!unique_columns.contains(&vec!["path".into()]));
        let foreign_key_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_violations, 0);
    }

    // Reopening runs the same eager migration path without duplicating rows.
    {
        let conn = db::open_db(&valid_project).unwrap();
        let subtrees = db::list_subtrees(&conn).unwrap();
        assert_eq!(subtrees.len(), 2);
        assert_eq!(metadata_value(&conn, "extra_roots"), None);
    }

    let normalized_project = tmp.path().join("normalized-project");
    let child = normalized_project.join("child");
    let extra = normalized_project.join("extra");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(extra.join("src")).unwrap();
    std::fs::write(extra.join("src/Legacy.kt"), "class Legacy\n").unwrap();
    let raw_extra = child
        .join("..")
        .join("extra")
        .to_string_lossy()
        .into_owned();
    let canonical_extra = db::normalize_root_for_storage(&extra);
    let normalized_db = tmp.path().join("normalized.sqlite");
    create_legacy_db(
        &normalized_db,
        &serde_json::to_string(&vec![raw_extra.clone(), canonical_extra.clone()]).unwrap(),
    );
    db_path_override.set(&normalized_db);

    {
        let conn = db::open_db(&normalized_project).unwrap();
        let subtrees = db::list_subtrees(&conn).unwrap();
        assert_eq!(subtrees.len(), 1, "canonical duplicates must collapse");
        assert_eq!(subtrees[0].canonical_path, canonical_extra);
        assert_eq!(subtrees[0].original_path, raw_extra);

        let resolver = PathResolver::from_conn(&normalized_project, &conn).with_decoration(false);
        assert_eq!(
            resolver.resolve_with_root("src/Legacy.kt", Some(&subtrees[0].canonical_path)),
            Path::new(&canonical_extra)
                .join("src/Legacy.kt")
                .to_string_lossy()
        );
    }

    let malformed_project = tmp.path().join("malformed-project");
    std::fs::create_dir(&malformed_project).unwrap();
    let malformed_db = tmp.path().join("malformed.sqlite");
    create_legacy_db(&malformed_db, r#"["/legacy/ok",]"#);
    db_path_override.set(&malformed_db);

    let error = match db::open_db(&malformed_project) {
        Ok(_) => panic!("malformed metadata.extra_roots unexpectedly migrated"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("metadata.extra_roots must be a JSON array of strings"),
        "unexpected migration error: {error:#}"
    );

    for args in [&["todo"][..], &["api", "missing-module"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&malformed_project)
            .env("AST_INDEX_DB_PATH", &malformed_db)
            .env_remove("KOTLIN_INDEX_DB_PATH")
            .args(args)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{} unexpectedly ignored migration failure: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("metadata.extra_roots must be a JSON array of strings"),
            "{} returned the wrong error: {stderr}",
            args.join(" ")
        );
    }

    // The schema changes and metadata update share the same transaction as
    // legacy-row conversion, so a parse error must roll everything back.
    {
        let conn = Connection::open(&malformed_db).unwrap();
        assert_eq!(
            metadata_value(&conn, "extra_roots").as_deref(),
            Some(r#"["/legacy/ok",]"#)
        );
        assert_eq!(
            metadata_value(&conn, "project_root").as_deref(),
            Some("legacy-project-root")
        );
        assert!(!schema_object_exists(&conn, "table", "subtrees"));
        assert!(!column_exists(&conn, "files", "root_path"));
        assert!(!column_exists(&conn, "symbols", "qualified_name"));
        assert!(!schema_object_exists(
            &conn,
            "index",
            "idx_symbols_qualified_name"
        ));
        assert_eq!(unique_index_columns(&conn, "files"), vec![vec!["path"]]);
        let preserved_symbol: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols WHERE file_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_symbol, 1);
    }

    let fresh_project = tmp.path().join("fresh-project");
    std::fs::create_dir(&fresh_project).unwrap();
    let fresh_db = tmp.path().join("fresh.sqlite");
    db_path_override.set(&fresh_db);

    // A brand-new file has no files/symbols tables yet. `open_db` must still
    // succeed and expose the migration-owned tables before `init_db` runs.
    let conn = db::open_db(&fresh_project).unwrap();
    assert!(db::list_subtrees(&conn).unwrap().is_empty());
    assert!(schema_object_exists(&conn, "table", "metadata"));
    assert!(schema_object_exists(&conn, "table", "subtrees"));
    assert!(!schema_object_exists(&conn, "table", "files"));
    assert!(!schema_object_exists(&conn, "table", "symbols"));

    db::init_db(&conn).unwrap();
    assert!(column_exists(&conn, "files", "root_path"));
    assert!(column_exists(&conn, "symbols", "qualified_name"));
    assert!(schema_object_exists(
        &conn,
        "index",
        "idx_symbols_qualified_name"
    ));
    drop(conn);

    let refs_project = tmp.path().join("legacy-refs-project");
    std::fs::create_dir(&refs_project).unwrap();
    let refs_db = tmp.path().join("legacy-refs.sqlite");
    db_path_override.set(&refs_db);
    {
        let conn = db::open_db(&refs_project).unwrap();
        db::init_db(&conn).unwrap();
        conn.execute_batch(
            r#"
            DROP INDEX idx_refs_name_file_line;
            CREATE INDEX idx_refs_name ON refs(name);
            "#,
        )
        .unwrap();
    }
    {
        let conn = db::open_db(&refs_project).unwrap();
        assert!(!schema_object_exists(&conn, "index", "idx_refs_name"));
        assert!(schema_object_exists(
            &conn,
            "index",
            "idx_refs_name_file_line"
        ));
        let columns = conn
            .prepare("SELECT name FROM pragma_index_info('idx_refs_name_file_line') ORDER BY seqno")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(columns, ["name", "file_id", "line"]);
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT file_id, line FROM refs WHERE name = 'Legacy'",
                [],
                |row| row.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_refs_name_file_line"),
            "legacy refs migration lost its covering plan: {plan}"
        );
    }

    let cli_project = tmp.path().join("cli-project");
    std::fs::create_dir(&cli_project).unwrap();
    let cli_db = tmp.path().join("cli.sqlite");
    create_legacy_db(&cli_db, r#"["/legacy/alpha","/legacy/beta"]"#);

    let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(&cli_project)
        .env("AST_INDEX_DB_PATH", &cli_db)
        .args(["--format", "json", "subtree", "list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "subtree list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "subtree list returned invalid JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(
        json,
        serde_json::json!([
            {
                "name": "alpha",
                "canonical_path": "/legacy/alpha",
                "original_path": "/legacy/alpha"
            },
            {
                "name": "beta",
                "canonical_path": "/legacy/beta",
                "original_path": "/legacy/beta"
            }
        ])
    );

    let auto_project = tmp.path().join("auto-sub-projects");
    let auto_cache = tmp.path().join("auto-cache");
    std::fs::create_dir(&auto_project).unwrap();
    std::fs::create_dir(&auto_cache).unwrap();
    for index in 0..20 {
        let sub_project = auto_project.join(format!("project-{index:02}"));
        std::fs::create_dir_all(sub_project.join("src")).unwrap();
        std::fs::write(
            sub_project.join("Cargo.toml"),
            format!(
                "[package]\nname = \"project-{index:02}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            sub_project.join("src/lib.rs"),
            format!("pub fn indexed_project_{index:02}() {{}}\n"),
        )
        .unwrap();
    }

    let auto_db = tmp.path().join("auto-sub-projects.sqlite");
    db_path_override.set(&auto_db);
    {
        let conn = db::open_db(&auto_project).unwrap();
        db::init_db(&conn).unwrap();
        assert_eq!(metadata_value(&conn, "extra_roots"), None);
        conn.execute("DROP TABLE subtrees", []).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(&auto_project)
        .env("AST_INDEX_DB_PATH", &auto_db)
        .env("AST_INDEX_CACHE_DIR", &auto_cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_MAX_FILES")
        .arg("rebuild")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "auto sub-project rebuild failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("Detected 20 sub-projects"),
        "rebuild did not take the auto sub-project shortcut: {stderr}"
    );
    assert!(
        !stdout.contains("no such table: subtrees") && !stderr.contains("no such table: subtrees"),
        "legacy schema error escaped migration\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let conn = Connection::open(&auto_db).unwrap();
    assert!(schema_object_exists(&conn, "table", "subtrees"));
    assert_eq!(metadata_value(&conn, "extra_roots"), None);
    let indexed_sub_projects: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path GLOB 'project-*/src/lib.rs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed_sub_projects, 20);
    for path in ["project-00/src/lib.rs", "project-19/src/lib.rs"] {
        let indexed: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE path = ?1)",
                [path],
                |row| row.get(0),
            )
            .unwrap();
        assert!(indexed, "missing indexed sub-project source: {path}");
    }
}
