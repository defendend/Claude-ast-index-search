//! Filesystem-level regressions for stale index-cache garbage collection.
//!
//! Every test supplies an isolated cache base and an injected clock. Nothing
//! here resolves or mutates the developer's real cache directory.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ast_index::db;
use tempfile::TempDir;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

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

    fs2::FileExt::unlock(&lease).unwrap();
    drop(lease);

    let after_release =
        db::gc_stale_caches_in(base.path(), None, db::STALE_CACHE_MAX_AGE, now).unwrap();
    assert_eq!(after_release, 1);
    assert!(!stale.exists());
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
