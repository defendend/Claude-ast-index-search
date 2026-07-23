use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use ast_index::db;
use rusqlite::TransactionBehavior;
use tempfile::TempDir;

fn environment_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct DbOverride {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl DbOverride {
    fn set(path: &Path) -> Self {
        let keys = [
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
            "AST_INDEX_CACHE_DIR",
        ];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("AST_INDEX_DB_PATH", path);
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_CACHE_DIR");
        Self { previous }
    }
}

impl Drop for DbOverride {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn create_live_index(root: &Path, sentinel: &str) {
    let conn = db::open_db(root).unwrap();
    db::init_db(&conn).unwrap();
    conn.execute(
        "INSERT INTO files(path, root_path, mtime, size) VALUES (?1, '', 1, 1)",
        [sentinel],
    )
    .unwrap();
}

fn create_staged_index(root: &Path, staged: &Path, sentinel: &str) {
    fs::create_dir(staged.parent().unwrap()).unwrap();
    let conn = db::open_staged_db(root, staged).unwrap();
    db::init_db(&conn).unwrap();
    conn.execute(
        "INSERT INTO files(path, root_path, mtime, size) VALUES (?1, '', 1, 1)",
        [sentinel],
    )
    .unwrap();
    db::seal_staged_db(conn, staged).unwrap();
}

fn run_reader(root: &Path, db_path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .env("AST_INDEX_DB_PATH", db_path)
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_CACHE_DIR")
        .env("AST_INDEX_DISABLE_GC", "1")
        .args(["query", "SELECT path FROM files ORDER BY path"])
        .output()
        .unwrap()
}

fn run_clear(root: &Path, db_path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .env("AST_INDEX_DB_PATH", db_path)
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_CACHE_DIR")
        .env("AST_INDEX_DISABLE_GC", "1")
        .arg("clear")
        .output()
        .unwrap()
}

fn run_update(root: &Path, db_path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .env("AST_INDEX_DB_PATH", db_path)
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_CACHE_DIR")
        .env("AST_INDEX_DISABLE_GC", "1")
        .arg("update")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn subprocess_reader_sees_old_generation_through_staged_build_then_new_after_publish() {
    let _lock = environment_lock();
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let live = temp.path().join("cache/index.db");
    let staged = temp.path().join("cache/.rebuild-test/index.db");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(live.parent().unwrap()).unwrap();
    let _override = DbOverride::set(&live);

    create_live_index(&root, "old-sentinel.rs");
    create_staged_index(&root, &staged, "new-sentinel.rs");

    let during_build = run_reader(&root, &live);
    assert!(
        during_build.status.success(),
        "reader failed while staged generation existed: {}",
        stderr(&during_build)
    );
    assert!(stdout(&during_build).contains("old-sentinel.rs"));
    assert!(!stdout(&during_build).contains("new-sentinel.rs"));

    let publisher = db::acquire_index_publication_guard(&root).unwrap();
    publisher.install_staged(&staged).unwrap();
    drop(publisher);

    let after_publish = run_reader(&root, &live);
    assert!(
        after_publish.status.success(),
        "reader failed after publication: {}",
        stderr(&after_publish)
    );
    assert!(stdout(&after_publish).contains("new-sentinel.rs"));
    assert!(!stdout(&after_publish).contains("old-sentinel.rs"));
}

#[test]
fn publication_contention_is_fast_and_never_reports_missing_or_partial_index() {
    let _lock = environment_lock();
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let live = temp.path().join("cache/index.db");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(live.parent().unwrap()).unwrap();
    let _override = DbOverride::set(&live);
    create_live_index(&root, "old-sentinel.rs");

    let publisher = db::acquire_index_publication_guard(&root).unwrap();
    let started = Instant::now();
    let blocked_reader = run_reader(&root, &live);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!blocked_reader.status.success());
    let error = stderr(&blocked_reader);
    assert!(error.contains("retry shortly"), "unexpected error: {error}");
    assert!(
        !error.contains("Index not found"),
        "unexpected error: {error}"
    );
    drop(publisher);

    let reader = db::open_db_leased(&root).unwrap();
    let started = Instant::now();
    let busy = match db::acquire_index_publication_guard(&root) {
        Ok(_) => panic!("publisher unexpectedly acquired over a live reader"),
        Err(error) => error,
    };
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(db::is_publication_busy(&busy), "unexpected error: {busy:#}");

    let started = Instant::now();
    let blocked_clear = run_clear(&root, &live);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!blocked_clear.status.success());
    assert!(stderr(&blocked_clear).contains("retry shortly"));
    let live_count: i64 = reader
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(live_count, 1, "failed clear changed the live generation");
    drop(reader);

    let publisher = db::acquire_index_publication_guard(&root).unwrap();
    let started = Instant::now();
    let busy = match db::open_db_leased(&root) {
        Ok(_) => panic!("reader unexpectedly opened under exclusive publication"),
        Err(error) => error,
    };
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(db::is_publication_busy(&busy), "unexpected error: {busy:#}");
    drop(publisher);
}

#[test]
fn ordinary_wal_writer_and_reader_remain_concurrent() {
    let _lock = environment_lock();
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let live = temp.path().join("cache/index.db");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(live.parent().unwrap()).unwrap();
    let _override = DbOverride::set(&live);
    create_live_index(&root, "committed.rs");

    let mut writer = db::open_db_leased(&root).unwrap();
    let transaction = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    transaction
        .execute(
            "INSERT INTO files(path, root_path, mtime, size) VALUES ('uncommitted.rs', '', 1, 1)",
            [],
        )
        .unwrap();

    let started = Instant::now();
    let reader = db::open_db_leased(&root).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    let paths = reader
        .prepare("SELECT path FROM files ORDER BY path")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(paths, ["committed.rs"]);

    drop(reader);
    transaction.rollback().unwrap();
}

#[test]
fn mutation_guard_serializes_update_without_blocking_readers() {
    let _lock = environment_lock();
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let live = temp.path().join("cache/index.db");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(live.parent().unwrap()).unwrap();
    let _override = DbOverride::set(&live);
    create_live_index(&root, "committed.rs");

    let mutation = db::acquire_rebuild_guard(&root).unwrap();
    let reader = run_reader(&root, &live);
    assert!(
        reader.status.success(),
        "mutation guard blocked an ordinary reader: {}",
        stderr(&reader)
    );

    let started = Instant::now();
    let update = run_update(&root, &live);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!update.status.success());
    assert!(
        stderr(&update).contains("Another rebuild is already running"),
        "unexpected error: {}",
        stderr(&update)
    );
    drop(mutation);
}
