use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ast_index::db;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use tempfile::TempDir;

const ACTIVITY_MARKER: &str = ".ast-index-access-v1";

struct CacheEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl CacheEnvironment {
    fn set(cache: &Path) -> Self {
        let keys = [
            "AST_INDEX_CACHE_DIR",
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
        ];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("AST_INDEX_CACHE_DIR", cache);
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        Self { previous }
    }
}

impl Drop for CacheEnvironment {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn index_sql(conn: &Connection, name: &str) -> Option<String> {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
        [name],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_os_string();
    path.push(suffix);
    path.into()
}

fn set_mtime(path: &Path, mtime: SystemTime) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn current_schema_reader_avoids_writer_lock_and_external_activity_protects_gc() {
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let project = temp.path().join("project");
    fs::create_dir(&cache).unwrap();
    fs::create_dir(&project).unwrap();
    let _environment = CacheEnvironment::set(&cache);

    let bootstrap = db::open_db_leased(&project).unwrap();
    db::init_db(&bootstrap).unwrap();
    drop(bootstrap);
    let db_path = db::get_db_path(&project).unwrap();
    let marker = db_path.parent().unwrap().join(ACTIVITY_MARKER);
    assert!(
        marker.is_file(),
        "successful open must create activity marker"
    );
    let old_marker_time = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
    set_mtime(&marker, old_marker_time);

    // Simulate the indexes created by older ast-index releases. These are
    // optional size migrations and must not make an otherwise-current reader
    // wait for a concurrent writer.
    let legacy = Connection::open(&db_path).unwrap();
    legacy
        .execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_files_root_path_path ON files(root_path, path);
            CREATE INDEX IF NOT EXISTS idx_modules_name ON modules(name);
            CREATE INDEX IF NOT EXISTS idx_refs_name ON refs(name);
            DROP INDEX IF EXISTS idx_symbols_qualified_name;
            CREATE INDEX idx_symbols_qualified_name ON symbols(qualified_name);
            "#,
        )
        .unwrap();
    drop(legacy);

    let mut writer = Connection::open(&db_path).unwrap();
    let writer_tx = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();

    let reader_started = Instant::now();
    let reader = db::open_db_leased(&project).unwrap();
    assert!(
        reader_started.elapsed() < Duration::from_secs(1),
        "current-schema reader waited for the writer lock"
    );
    let symbol_count: i64 = reader
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .unwrap();
    assert_eq!(symbol_count, 0);
    assert!(index_sql(&reader, "idx_files_root_path_path").is_some());
    assert!(index_sql(&reader, "idx_modules_name").is_some());
    assert!(index_sql(&reader, "idx_refs_name").is_some());
    assert!(
        fs::metadata(&marker).unwrap().modified().unwrap() > old_marker_time,
        "current-schema read must refresh external activity without writing SQLite"
    );
    drop(reader);

    writer_tx.rollback().unwrap();
    drop(writer);

    // With no writer, a later open performs the deferred size-only cleanup.
    let cleaned = db::open_db_leased(&project).unwrap();
    assert!(index_sql(&cleaned, "idx_files_root_path_path").is_none());
    assert!(index_sql(&cleaned, "idx_modules_name").is_none());
    assert!(index_sql(&cleaned, "idx_refs_name").is_none());
    let qualified_sql = index_sql(&cleaned, "idx_symbols_qualified_name").unwrap();
    assert!(
        qualified_sql
            .to_ascii_lowercase()
            .contains("where qualified_name is not null"),
        "unexpected qualified-name index: {qualified_sql}"
    );
    drop(cleaned);

    let now = SystemTime::now();
    let stale = now - db::STALE_CACHE_MAX_AGE - Duration::from_secs(24 * 60 * 60);
    for path in [
        db_path.clone(),
        append_suffix(&db_path, "-wal"),
        append_suffix(&db_path, "-shm"),
        append_suffix(&db_path, "-journal"),
        db_path.with_extension("db.swap"),
        db_path.with_extension("db.swap-wal"),
        db_path.with_extension("db.swap-shm"),
        db_path.with_extension("db.swap-journal"),
    ] {
        if path.is_file() {
            set_mtime(&path, stale);
        }
    }

    let kept = db::gc_stale_caches_in(&cache, None, db::STALE_CACHE_MAX_AGE, now).unwrap();
    assert_eq!(kept, 0);
    assert!(
        db_path.is_file(),
        "fresh activity marker must protect cache"
    );

    set_mtime(&marker, stale);
    let removed = db::gc_stale_caches_in(&cache, None, db::STALE_CACHE_MAX_AGE, now).unwrap();
    assert_eq!(removed, 1);
    assert!(!db_path.exists());

    // A legacy rollback-journal database also stays readable: changing the
    // persistent journal mode is optional and must be deferred while busy.
    let rollback_project = temp.path().join("rollback-project");
    fs::create_dir(&rollback_project).unwrap();
    let rollback_bootstrap = db::open_db_leased(&rollback_project).unwrap();
    db::init_db(&rollback_bootstrap).unwrap();
    drop(rollback_bootstrap);
    let rollback_path = db::get_db_path(&rollback_project).unwrap();
    let rollback_mode = Connection::open(&rollback_path).unwrap();
    let mode: String = rollback_mode
        .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    drop(rollback_mode);

    let mut rollback_writer = Connection::open(&rollback_path).unwrap();
    let rollback_tx = rollback_writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let rollback_reader = db::open_db_leased(&rollback_project).unwrap();
    let mode: String = rollback_reader
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    drop(rollback_reader);
    rollback_tx.rollback().unwrap();
    drop(rollback_writer);

    let upgraded = db::open_db_leased(&rollback_project).unwrap();
    let mode: String = upgraded
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}
