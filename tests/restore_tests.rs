use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ast_index::db;
use rusqlite::Connection;
use tempfile::TempDir;

struct RestoreHarness {
    _temp: TempDir,
    project: PathBuf,
    cache: PathBuf,
}

impl RestoreHarness {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&cache).unwrap();
        Self {
            _temp: temp,
            project,
            cache,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ast-index"));
        command
            .current_dir(&self.project)
            .env("AST_INDEX_CACHE_DIR", &self.cache)
            .env_remove("AST_INDEX_DB_PATH")
            .env_remove("KOTLIN_INDEX_DB_PATH")
            .env("AST_INDEX_DISABLE_GC", "1");
        command
    }

    fn run(&self, args: &[&OsStr]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn live_db(&self) -> PathBuf {
        let output = self.run(&[OsStr::new("db-path")]);
        assert_success(&output);
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    fn create_live_index(&self, marker: &str) -> PathBuf {
        let output = self.run(&[OsStr::new("rebuild")]);
        assert_success(&output);
        let live_db = self.live_db();
        let conn = Connection::open(&live_db).unwrap();
        db::upsert_file(&conn, marker, 1, 1).unwrap();
        live_db
    }

    fn restore(&self, source: &Path) -> Output {
        self.run(&[OsStr::new("restore"), source.as_os_str()])
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn create_source(path: &Path, marker: &str) {
    let conn = Connection::open(path).unwrap();
    db::init_db(&conn).unwrap();
    db::upsert_file(&conn, marker, 1, 1).unwrap();
}

fn create_minimal_source_without_derived_schema(path: &Path, marker: &str) {
    let conn = Connection::open(path).unwrap();
    db::init_db_for_rebuild(&conn).unwrap();
    db::upsert_file(&conn, marker, 1, 1).unwrap();
}

fn file_paths(db_path: &Path) -> Vec<String> {
    let conn = Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT path FROM files ORDER BY path")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn assert_no_restore_artifacts(live_db: &Path) {
    let parent = live_db.parent().unwrap();
    let staging = fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(".restore-"))
        .collect::<Vec<_>>();
    assert!(staging.is_empty(), "staging artifacts remain: {staging:?}");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(
            !live_db.with_extension(format!("db.swap{suffix}")).exists(),
            "swap artifact remains for suffix {suffix}"
        );
    }
    for extension in [
        "db.swap-pending",
        "db.publish-state-v1",
        "db.publish-commit-v1",
    ] {
        assert!(
            !live_db.with_extension(extension).exists(),
            "publication artifact remains: {extension}"
        );
    }
}

#[test]
fn restore_rejects_the_live_database_without_deleting_it() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");

    let output = harness.restore(&live_db);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("restore source is the live index database"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(file_paths(&live_db), ["old.rs"]);
    assert_no_restore_artifacts(&live_db);
}

#[cfg(unix)]
#[test]
fn restore_rejects_a_hard_link_to_the_live_database() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let alias = live_db.parent().unwrap().join("live-hardlink.db");
    fs::hard_link(&live_db, &alias).unwrap();

    let output = harness.restore(&alias);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("restore source is the live index database"));
    assert_eq!(file_paths(&live_db), ["old.rs"]);
    assert_no_restore_artifacts(&live_db);
}

#[test]
fn corrupt_restore_source_preserves_the_live_database() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let corrupt = harness.project.join("corrupt.db");
    fs::write(&corrupt, b"not a sqlite database").unwrap();

    let output = harness.restore(&corrupt);

    assert!(!output.status.success());
    assert_eq!(file_paths(&live_db), ["old.rs"]);
    assert_no_restore_artifacts(&live_db);
}

#[test]
fn unrelated_sqlite_schema_is_rejected_before_the_live_database_changes() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let unrelated = harness.project.join("unrelated.db");
    let conn = Connection::open(&unrelated).unwrap();
    conn.execute("CREATE TABLE unrelated (value TEXT)", [])
        .unwrap();
    drop(conn);

    let output = harness.restore(&unrelated);

    assert!(!output.status.success());
    assert_eq!(file_paths(&live_db), ["old.rs"]);
    assert_no_restore_artifacts(&live_db);
}

#[test]
fn restore_rejects_index_without_fts_and_sync_triggers() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let incomplete = harness.project.join("incomplete.db");
    create_minimal_source_without_derived_schema(&incomplete, "incomplete.rs");

    let output = harness.restore(&incomplete);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing symbols_fts virtual table"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(file_paths(&live_db), ["old.rs"]);
    assert_no_restore_artifacts(&live_db);
}

#[test]
fn valid_restore_atomically_replaces_the_live_database() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let source = harness.project.join("backup.db");
    create_source(&source, "restored.rs");

    let output = harness.restore(&source);

    assert_success(&output);
    assert_eq!(file_paths(&live_db), ["restored.rs"]);
    let conn = Connection::open(&live_db).unwrap();
    let stored_root: String = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'project_root'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored_root,
        db::normalize_root_for_storage(&harness.project)
    );
    assert_no_restore_artifacts(&live_db);
}

#[test]
fn restore_reaches_recovery_when_publication_marker_blocks_read_discovery() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let source = harness.project.join("backup-after-crash.db");
    create_source(&source, "restored-after-crash.rs");

    fs::copy(&live_db, live_db.with_extension("db.swap")).unwrap();
    fs::write(
        live_db.with_extension("db.publish-state-v1"),
        br#"{"version":1,"token":"1-1-1","operation":"install","artifacts":[true,false,false,false]}"#,
    )
    .unwrap();

    let output = harness.restore(&source);

    assert_success(&output);
    assert_eq!(file_paths(&live_db), ["restored-after-crash.rs"]);
    assert_no_restore_artifacts(&live_db);
}

#[test]
fn restore_captures_rows_committed_only_to_the_source_wal() {
    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let source = harness.project.join("wal-backup.db");
    let source_conn = Connection::open(&source).unwrap();
    let mode: String = source_conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    source_conn
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    db::init_db(&source_conn).unwrap();
    db::upsert_file(&source_conn, "wal-only.rs", 1, 1).unwrap();
    let wal = source.with_extension("db-wal");
    assert!(fs::metadata(&wal).unwrap().len() > 0);

    let output = harness.restore(&source);

    assert_success(&output);
    assert_eq!(file_paths(&live_db), ["wal-only.rs"]);
    assert_no_restore_artifacts(&live_db);
    drop(source_conn);
}

#[cfg(unix)]
#[test]
fn restore_rejects_a_symlink_source_without_touching_the_live_database() {
    use std::os::unix::fs::symlink;

    let harness = RestoreHarness::new();
    let live_db = harness.create_live_index("old.rs");
    let source = harness.project.join("backup.db");
    let alias = harness.project.join("backup-link.db");
    create_source(&source, "restored.rs");
    symlink(&source, &alias).unwrap();

    let output = harness.restore(&alias);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("restore source is not a regular file")
    );
    assert_eq!(file_paths(&live_db), ["old.rs"]);
    assert_no_restore_artifacts(&live_db);
}
