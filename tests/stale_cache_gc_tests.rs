//! Filesystem-level regressions for stale index-cache garbage collection.
//!
//! Every test supplies an isolated cache base and an injected clock. Nothing
//! here resolves or mutates the developer's real cache directory.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ast_index::db;
use tempfile::TempDir;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

const CHILD_LOCK_PATH: &str = "AST_INDEX_GC_TEST_CHILD_LOCK_PATH";
const CHILD_PROJECT: &str = "AST_INDEX_GC_TEST_CHILD_PROJECT";
const CHILD_READY: &str = "AST_INDEX_GC_TEST_CHILD_READY";
const CHILD_RELEASE: &str = "AST_INDEX_GC_TEST_CHILD_RELEASE";

fn test_now() -> SystemTime {
    // Whole seconds make exact-boundary assertions independent of how a
    // filesystem rounds sub-second mtimes.
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn at_age(now: SystemTime, age: Duration) -> SystemTime {
    now.checked_sub(age).unwrap()
}

fn set_mtime(path: &Path, mtime: SystemTime) {
    let file = OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(mtime).unwrap();
}

fn write_activity_file(dir: &Path, name: &str, mtime: SystemTime) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, b"test cache activity").unwrap();
    set_mtime(&path, mtime);
    path
}

fn make_cache(base: &Path, key: &str, anchor: &str, mtime: SystemTime) -> PathBuf {
    let dir = base.join(key);
    write_activity_file(&dir, anchor, mtime);
    dir
}

fn assert_empty_dir(path: &Path) {
    assert!(path.is_dir(), "{} should be a directory", path.display());
    assert_eq!(
        fs::read_dir(path).unwrap().count(),
        0,
        "{} should be empty",
        path.display()
    );
}

fn leases(base: &Path) -> PathBuf {
    base.join(".leases")
}

/// Create empty `.leases` files the way ast-index leaves them behind.
fn touch_leases(base: &Path, names: &[&str]) {
    fs::create_dir_all(leases(base)).unwrap();
    for name in names {
        File::create(leases(base).join(name)).unwrap();
    }
}

fn lock_pair(key: &str) -> [String; 2] {
    [format!("{key}.lock"), format!("{key}.publish.lock")]
}

fn touch_lock_pair(base: &Path, key: &str) {
    let [lock, publish] = lock_pair(key);
    touch_leases(base, &[&lock, &publish]);
}

fn lease_exists(base: &Path, name: &str) -> bool {
    fs::symlink_metadata(leases(base).join(name)).is_ok()
}

fn open_lease(base: &Path, name: &str) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(leases(base).join(name))
        .unwrap()
}

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
    panic!("test child process did not exit");
}

fn child_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

/// Re-run one test of this binary as a child process with extra variables.
fn spawn_test_child(test_name: &str, env: &[(&str, &Path)]) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", test_name, "--nocapture"]);
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().unwrap()
}

/// Hold a shared flock on `path` from another thread until the returned
/// sender is dropped or signalled.
fn hold_shared_lock_in_thread(path: PathBuf) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        fs2::FileExt::lock_shared(&file).unwrap();
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
        drop(file);
    });
    ready_rx.recv().unwrap();
    (release_tx, holder)
}

#[test]
fn deletes_stale_cache_and_keeps_fresh_cache() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let fresh = make_cache(
        base.path(),
        "a1",
        "index.db",
        at_age(now, Duration::from_secs(60)),
    );
    let stale = make_cache(
        base.path(),
        "0123456789abcdef",
        "index.db",
        at_age(now, db::STALE_CACHE_MAX_AGE + Duration::from_secs(1)),
    );

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 1);
    assert!(fresh.is_dir());
    assert!(!stale.exists());
    assert_empty_dir(&base.path().join(".gc-trash"));
}

#[test]
fn keeps_exact_fourteen_day_boundary_and_deletes_one_second_past_it() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let fourteen_days = DAY * 14;
    assert_eq!(db::STALE_CACHE_MAX_AGE, fourteen_days);

    let boundary = make_cache(base.path(), "c3", "index.db", at_age(now, fourteen_days));
    let over_boundary = make_cache(
        base.path(),
        "d4",
        "index.db",
        at_age(now, fourteen_days + Duration::from_secs(1)),
    );

    let removed = db::gc_stale_caches_in(base.path(), None, fourteen_days, now).unwrap();

    assert_eq!(removed, 1);
    assert!(boundary.is_dir());
    assert!(!over_boundary.exists());
}

#[test]
fn keeps_cache_with_future_activity_timestamp() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let future = make_cache(base.path(), "e5", "index.db", now.checked_add(DAY).unwrap());

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 0);
    assert!(future.is_dir());
}

#[test]
fn never_deletes_kept_cache_even_when_stale() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);
    let kept = make_cache(base.path(), "cafe", "index.db", stale_mtime);
    let other = make_cache(base.path(), "dead", "index.db", stale_mtime);

    let removed =
        db::gc_stale_caches_in(base.path(), Some("cafe"), db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 1);
    assert!(kept.is_dir());
    assert!(!other.exists());
}

#[test]
fn collects_legacy_cache_with_only_stale_main_database() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let legacy = make_cache(
        base.path(),
        "1",
        "index.db",
        at_age(now, db::STALE_CACHE_MAX_AGE + Duration::from_secs(1)),
    );
    assert_eq!(fs::read_dir(&legacy).unwrap().count(), 1);

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 1);
    assert!(!legacy.exists());
}

#[test]
fn recent_main_sidecar_keeps_cache_with_stale_main_database() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);
    let recent_mtime = at_age(now, Duration::from_secs(30));
    let wal_cache = make_cache(base.path(), "a11ce", "index.db", stale_mtime);
    write_activity_file(&wal_cache, "index.db-wal", recent_mtime);
    let shm_cache = make_cache(base.path(), "a11cf", "index.db", stale_mtime);
    write_activity_file(&shm_cache, "index.db-shm", recent_mtime);

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 0);
    assert!(wal_cache.is_dir());
    assert!(shm_cache.is_dir());
}

#[test]
fn recent_journal_keeps_stale_live_and_swap_caches() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);
    let recent_mtime = at_age(now, Duration::from_secs(30));

    let live_cache = make_cache(base.path(), "a11d0", "index.db", stale_mtime);
    write_activity_file(&live_cache, "index.db-journal", recent_mtime);
    let swap_cache = make_cache(base.path(), "a11d1", "index.db.swap", stale_mtime);
    write_activity_file(&swap_cache, "index.db.swap-journal", recent_mtime);

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 0);
    assert!(live_cache.is_dir());
    assert!(swap_cache.is_dir());
}

#[test]
fn handles_swap_only_cache_and_swap_sidecar_activity() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);
    let recent_mtime = at_age(now, Duration::from_secs(30));

    let stale_swap = make_cache(base.path(), "5a1e", "index.db.swap", stale_mtime);
    let active_wal = make_cache(base.path(), "5a1f", "index.db.swap", stale_mtime);
    write_activity_file(&active_wal, "index.db.swap-wal", recent_mtime);
    let active_shm = make_cache(base.path(), "5a20", "index.db.swap", stale_mtime);
    write_activity_file(&active_shm, "index.db.swap-shm", recent_mtime);

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    // A swap without a publication marker has ambiguous ownership. Recovery
    // fails closed, so GC must preserve it indefinitely instead of guessing.
    assert_eq!(removed, 0);
    assert!(stale_swap.exists());
    assert!(active_wal.is_dir());
    assert!(active_shm.is_dir());
}

#[test]
fn active_publication_marker_is_never_collected_by_age() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);
    let publishing = make_cache(base.path(), "5a21", "index.db", stale_mtime);
    write_activity_file(&publishing, "index.db.publish-state-v1", stale_mtime);

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 0);
    assert!(publishing.is_dir());
}

#[test]
fn ignores_invalid_names_foreign_dirs_and_non_directory_entries() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);

    let invalid_names = ["not-hex", "ABC", "0123456789abcdef0"];
    for name in invalid_names {
        make_cache(base.path(), name, "index.db", stale_mtime);
    }

    let foreign = base.path().join("face");
    write_activity_file(&foreign, "notes.txt", stale_mtime);

    let non_directory = base.path().join("bead");
    fs::write(&non_directory, b"not a cache directory").unwrap();
    set_mtime(&non_directory, stale_mtime);

    let non_file_anchor = base.path().join("dad");
    fs::create_dir_all(non_file_anchor.join("index.db")).unwrap();

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 0);
    for name in invalid_names {
        assert!(base.path().join(name).is_dir());
    }
    assert!(foreign.is_dir());
    assert!(non_directory.is_file());
    assert!(non_file_anchor.is_dir());
}

#[cfg(unix)]
#[test]
fn ignores_top_level_symlink_even_when_target_is_a_stale_cache() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("base");
    let target = temp.path().join("outside-cache");
    fs::create_dir_all(&base).unwrap();
    write_activity_file(
        &target,
        "index.db",
        at_age(test_now(), db::STALE_CACHE_MAX_AGE + DAY),
    );
    let link = base.join("abcd");
    symlink(&target, &link).unwrap();

    let removed = db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(target.join("index.db").is_file());
}

#[cfg(unix)]
#[test]
fn ignores_cache_whose_index_database_is_a_symlink() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("base");
    let cache = base.join("c0de");
    fs::create_dir_all(&cache).unwrap();
    let target = write_activity_file(
        temp.path(),
        "outside.db",
        at_age(test_now(), db::STALE_CACHE_MAX_AGE + DAY),
    );
    let link = cache.join("index.db");
    symlink(&target, &link).unwrap();

    let removed = db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    assert!(cache.is_dir());
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(target.is_file());
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_gc_trash_directory() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("base");
    let outside = temp.path().join("outside-trash");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(outside.join("must-survive")).unwrap();
    symlink(&outside, base.join(".gc-trash")).unwrap();
    let stale = make_cache(
        &base,
        "feed",
        "index.db",
        at_age(test_now(), db::STALE_CACHE_MAX_AGE + DAY),
    );

    let removed = db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    assert!(stale.is_dir());
    assert!(outside.join("must-survive").is_dir());
}

#[test]
fn held_shared_project_lease_defers_collection_until_released() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let key = "1ea5e";
    let stale = make_cache(
        base.path(),
        key,
        "index.db",
        at_age(now, db::STALE_CACHE_MAX_AGE + DAY),
    );
    let leases = base.path().join(".leases");
    fs::create_dir_all(&leases).unwrap();
    let lease: File = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(leases.join(format!("{key}.lock")))
        .unwrap();
    fs2::FileExt::lock_shared(&lease).unwrap();

    let while_held =
        db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();
    assert_eq!(while_held, 0);
    assert!(stale.is_dir());
    assert!(leases.join(format!("{key}.lock")).is_file());

    fs2::FileExt::unlock(&lease).unwrap();
    drop(lease);

    let after_release =
        db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();
    assert_eq!(after_release, 1);
    assert!(!stale.exists());
    assert!(!leases.join(format!("{key}.lock")).exists());
}

#[test]
fn cleans_crash_leftover_tombstone_without_counting_it_as_new_removal() {
    let base = TempDir::new().unwrap();
    let tombstone = base.path().join(".gc-trash").join("dead.123.0");
    fs::create_dir_all(tombstone.join("nested")).unwrap();
    fs::write(tombstone.join("nested").join("index.db"), b"leftover").unwrap();

    let removed =
        db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    assert!(!tombstone.exists());
    assert_empty_dir(&base.path().join(".gc-trash"));
}

#[test]
fn leaves_foreign_directory_inside_gc_trash_untouched() {
    let base = TempDir::new().unwrap();
    let foreign = base.path().join(".gc-trash").join("notes-not-a-tombstone");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("important.txt"), b"keep").unwrap();

    let removed =
        db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    assert!(foreign.join("important.txt").is_file());
}

#[test]
fn missing_base_directory_is_a_noop() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("does-not-exist");

    let removed =
        db::gc_stale_caches_in(&missing, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    assert!(!missing.exists());
}

#[test]
fn collected_cache_loses_its_lease_locks() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let stale_mtime = at_age(now, db::STALE_CACHE_MAX_AGE + DAY);
    let fresh = make_cache(
        base.path(),
        "a1",
        "index.db",
        at_age(now, Duration::from_secs(60)),
    );
    let stale = make_cache(base.path(), "0123456789abcdef", "index.db", stale_mtime);
    let never_leased = make_cache(base.path(), "fed", "index.db", stale_mtime);
    touch_lock_pair(base.path(), "a1");
    touch_lock_pair(base.path(), "0123456789abcdef");

    let removed = db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 2);
    assert!(fresh.is_dir());
    assert!(!stale.exists() && !never_leased.exists());
    for key in ["0123456789abcdef", "fed"] {
        for name in lock_pair(key) {
            assert!(
                !lease_exists(base.path(), &name),
                "{name} outlived its cache"
            );
        }
    }
    for name in lock_pair("a1") {
        assert!(
            lease_exists(base.path(), &name),
            "{name} of a live cache was removed"
        );
    }
    assert!(lease_exists(base.path(), "layout.lock"));
}

#[test]
fn orphaned_lease_locks_are_removed_regardless_of_age() {
    let base = TempDir::new().unwrap();
    let now = test_now();
    let fresh = make_cache(
        base.path(),
        "a1",
        "index.db",
        at_age(now, Duration::from_secs(60)),
    );
    touch_lock_pair(base.path(), "a1");
    let orphans = ["0a", "0b", "0123456789abcdef", "0c", "0d"];
    for key in &orphans[..3] {
        touch_lock_pair(base.path(), key);
    }
    touch_leases(base.path(), &["0c.lock", "0d.publish.lock"]);
    // The kept key keeps its locks, and so does a key whose name a plain file
    // still occupies in the base.
    touch_lock_pair(base.path(), "cafe");
    fs::write(base.path().join("bead"), b"not a cache directory").unwrap();
    touch_lock_pair(base.path(), "bead");
    let unrelated = [
        "layout.lock",
        "0a.owner.4242.1.json",
        ".owner-manifest.4242.1.tmp",
        "notes.lock",
        "ABC.lock",
        "0123456789abcdef0.lock",
        "0e.lock.tmp",
    ];
    touch_leases(base.path(), &unrelated);

    let removed =
        db::gc_stale_caches_in(base.path(), Some("cafe"), db::STALE_CACHE_MAX_AGE, now).unwrap();

    assert_eq!(removed, 0);
    assert!(fresh.is_dir());
    for key in orphans {
        for name in lock_pair(key) {
            assert!(
                !lease_exists(base.path(), &name),
                "orphaned {name} survived"
            );
        }
    }
    for key in ["a1", "cafe", "bead"] {
        for name in lock_pair(key) {
            assert!(lease_exists(base.path(), &name), "{name} was removed");
        }
    }
    for name in unrelated {
        assert!(lease_exists(base.path(), name), "{name} was removed");
    }
}

#[test]
fn orphaned_locks_held_by_another_process_or_thread_survive_until_released() {
    if let Some(lock_path) = child_env(CHILD_LOCK_PATH) {
        let ready = child_env(CHILD_READY).unwrap();
        let release = child_env(CHILD_RELEASE).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        fs2::FileExt::lock_shared(&file).unwrap();
        fs::write(&ready, b"ready").unwrap();
        assert!(wait_for_path(&release, Duration::from_secs(30)));
        drop(file);
        return;
    }

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("cache");
    let (process_key, thread_key, publication_key) = ("0e", "0f", "10");
    for key in [process_key, thread_key, publication_key] {
        touch_lock_pair(&base, key);
    }
    let ready = temp.path().join("ready");
    let release = temp.path().join("release");
    let mut child = spawn_test_child(
        "orphaned_locks_held_by_another_process_or_thread_survive_until_released",
        &[
            (
                CHILD_LOCK_PATH,
                &leases(&base).join(format!("{process_key}.lock")),
            ),
            (CHILD_READY, &ready),
            (CHILD_RELEASE, &release),
        ],
    );
    assert!(
        wait_for_path(&ready, Duration::from_secs(30)),
        "lock-holding child did not start"
    );
    let (release_thread, thread) =
        hold_shared_lock_in_thread(leases(&base).join(format!("{thread_key}.lock")));
    let (release_publication, publication) =
        hold_shared_lock_in_thread(leases(&base).join(format!("{publication_key}.publish.lock")));

    let removed = db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    assert_eq!(removed, 0);
    for key in [process_key, thread_key, publication_key] {
        for name in lock_pair(key) {
            assert!(lease_exists(&base, &name), "held {name} was removed");
        }
    }

    fs::write(&release, b"release").unwrap();
    let status = wait_for_exit(&mut child, Duration::from_secs(30));
    assert!(status.success(), "lock-holding child failed: {status}");
    release_thread.send(()).unwrap();
    thread.join().unwrap();
    release_publication.send(()).unwrap();
    publication.join().unwrap();

    db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();
    for key in [process_key, thread_key, publication_key] {
        for name in lock_pair(key) {
            assert!(!lease_exists(&base, &name), "released {name} survived");
        }
    }
}

#[test]
fn lease_sweep_waits_for_the_cache_layout_lock() {
    let base = TempDir::new().unwrap();
    touch_lock_pair(base.path(), "0a");
    touch_leases(base.path(), &["layout.lock"]);
    let layout = open_lease(base.path(), "layout.lock");
    fs2::FileExt::lock_exclusive(&layout).unwrap();

    db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();
    for name in lock_pair("0a") {
        assert!(
            lease_exists(base.path(), &name),
            "{name} removed without the layout lock"
        );
    }

    drop(layout);
    db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();
    for name in lock_pair("0a") {
        assert!(
            !lease_exists(base.path(), &name),
            "orphaned {name} survived"
        );
    }
    assert!(lease_exists(base.path(), "layout.lock"));
}

#[cfg(unix)]
#[test]
fn symlinked_leases_directory_is_not_swept() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("base");
    let outside = temp.path().join("outside-leases");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&outside).unwrap();
    for name in lock_pair("0a") {
        File::create(outside.join(name)).unwrap();
    }
    symlink(&outside, leases(&base)).unwrap();

    db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    for name in lock_pair("0a") {
        assert!(
            outside.join(&name).is_file(),
            "{name} removed through a symlink"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_lease_locks_and_cache_entries_are_left_alone() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("base");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let outside_lock = outside.join("target.lock");
    let outside_publication = outside.join("target.publish.lock");
    File::create(&outside_lock).unwrap();
    File::create(&outside_publication).unwrap();

    touch_leases(&base, &["0a.publish.lock", "0b.lock"]);
    symlink(&outside_lock, leases(&base).join("0a.lock")).unwrap();
    symlink(&outside_publication, leases(&base).join("0b.publish.lock")).unwrap();
    symlink(&outside, base.join("feed")).unwrap();
    touch_lock_pair(&base, "feed");

    db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, test_now()).unwrap();

    for key in ["0a", "0b", "feed"] {
        for name in lock_pair(key) {
            assert!(lease_exists(&base, &name), "{name} was removed");
        }
    }
    assert!(outside_lock.is_file() && outside_publication.is_file());
}

// Removing a directory that holds an open SQLite database is Unix-only.
#[cfg(unix)]
#[test]
fn concurrent_leased_opens_never_split_a_key_lock_across_inodes() {
    const ITERATIONS: usize = 100;

    if let Some(project) = child_env(CHILD_PROJECT) {
        let base = child_env("AST_INDEX_CACHE_DIR").unwrap();
        let db_path = db::get_db_path(&project).unwrap();
        let cache_dir = db_path.parent().unwrap().to_path_buf();
        let key = cache_dir.file_name().unwrap().to_str().unwrap().to_owned();
        for iteration in 0..ITERATIONS {
            let connection = db::open_db_leased(&project)
                .unwrap_or_else(|error| panic!("open {iteration} failed: {error:#}"));
            // Every other cache vanishes while its lease is still held, and
            // the sweeping parent gets time to see the key as orphaned.
            let removed_while_leased = iteration % 2 == 1;
            if removed_while_leased {
                fs::remove_dir_all(&cache_dir).unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
            // A path that no longer names the inode this lease locks would
            // let the next opener lock a different file for the same key.
            for name in lock_pair(&key) {
                let probe = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(leases(&base).join(&name))
                    .unwrap_or_else(|error| {
                        panic!("open {iteration}: held {name} disappeared: {error}")
                    });
                assert!(
                    fs2::FileExt::try_lock_exclusive(&probe).is_err(),
                    "open {iteration}: {name} no longer names the held lock"
                );
            }
            drop(connection);
            if !removed_while_leased {
                fs::remove_dir_all(&cache_dir).unwrap();
            }
        }
        return;
    }

    let temp = TempDir::new().unwrap();
    let base = temp.path().join("cache");
    let project = temp.path().join("project");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&project).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "concurrent_leased_opens_never_split_a_key_lock_across_inodes",
            "--nocapture",
        ])
        .env(CHILD_PROJECT, &project)
        .env("AST_INDEX_CACHE_DIR", &base)
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .spawn()
        .unwrap();

    let started = Instant::now();
    let mut sweeps = 0_u64;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(120) {
            child.kill().unwrap();
            panic!("leased-open child did not finish");
        }
        db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, SystemTime::now()).unwrap();
        sweeps += 1;
        std::thread::sleep(Duration::from_micros(200));
    };
    assert!(status.success(), "leased-open child failed: {status}");
    assert!(sweeps > 0);

    db::gc_stale_caches_in(&base, None, db::STALE_CACHE_MAX_AGE, SystemTime::now()).unwrap();
    let leftover: Vec<String> = fs::read_dir(leases(&base))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".lock") && name != "layout.lock")
        .collect();
    assert!(leftover.is_empty(), "orphaned locks survived: {leftover:?}");
}
