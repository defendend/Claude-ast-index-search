use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime};

use ast_index::db;
use tempfile::TempDir;

const CHILD_MODE: &str = "AST_INDEX_OPEN_DB_GC_CHILD";
const CHILD_PROJECT: &str = "AST_INDEX_OPEN_DB_GC_PROJECT";
const CHILD_CACHE: &str = "AST_INDEX_OPEN_DB_GC_CACHE";
const CHILD_READY: &str = "AST_INDEX_OPEN_DB_GC_READY";
const CHILD_RELEASE: &str = "AST_INDEX_OPEN_DB_GC_RELEASE";

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    panic!("raw open_db child did not exit");
}

fn child_path(key: &str) -> PathBuf {
    std::env::var_os(key)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("missing child path {key}"))
}

#[test]
fn raw_open_db_process_lease_blocks_gc_until_process_exit() {
    if std::env::var_os(CHILD_MODE).is_some() {
        let project = child_path(CHILD_PROJECT);
        let cache = child_path(CHILD_CACHE);
        let ready = child_path(CHILD_READY);
        let release = child_path(CHILD_RELEASE);
        std::env::set_var("AST_INDEX_CACHE_DIR", &cache);
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");

        let connection = db::open_db(&project).unwrap();
        db::init_db(&connection).unwrap();
        let db_path = db::get_db_path(&project).unwrap();
        let pending_ready = ready.with_extension("tmp");
        fs::write(&pending_ready, db_path.to_string_lossy().as_bytes()).unwrap();
        fs::rename(pending_ready, &ready).unwrap();
        assert!(wait_for_path(&release, Duration::from_secs(10)));
        drop(connection);
        return;
    }

    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let cache = temp.path().join("cache");
    let ready = temp.path().join("ready");
    let release = temp.path().join("release");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&cache).unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "raw_open_db_process_lease_blocks_gc_until_process_exit",
            "--nocapture",
        ])
        .env(CHILD_MODE, "1")
        .env(CHILD_PROJECT, &project)
        .env(CHILD_CACHE, &cache)
        .env(CHILD_READY, &ready)
        .env(CHILD_RELEASE, &release)
        .spawn()
        .unwrap();

    assert!(
        wait_for_path(&ready, Duration::from_secs(10)),
        "raw open_db child did not become ready"
    );
    let db_path = PathBuf::from(fs::read_to_string(&ready).unwrap());
    let future = SystemTime::now()
        .checked_add(db::STALE_CACHE_MAX_AGE + Duration::from_secs(60))
        .unwrap();

    let kept = db::gc_stale_caches_in(&cache, None, db::STALE_CACHE_MAX_AGE, future).unwrap();
    assert_eq!(kept, 0);
    assert!(
        db_path.is_file(),
        "GC removed cache while raw handle was live"
    );

    fs::write(&release, b"release").unwrap();
    let status = wait_for_exit(&mut child, Duration::from_secs(10));
    assert!(status.success(), "raw open_db child failed: {status}");

    let removed = db::gc_stale_caches_in(&cache, None, db::STALE_CACHE_MAX_AGE, future).unwrap();
    assert_eq!(removed, 1);
    assert!(
        !db_path.exists(),
        "process-lifetime lease survived child exit"
    );
}
