//! Regression coverage for whole-directory cache-path migration.
//!
//! This target intentionally contains one test because cache and database
//! overrides are process-global. Both scenarios therefore run serially under
//! one environment guard and only touch an isolated temporary cache.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use ast_index::db;
use rusqlite::{params, Connection};
use tempfile::TempDir;

const MARKER_CONTENTS: &[u8] = b"directory migration marker\n";
const SIDECAR_NAME: &str = "index.db.sidecar";
const SIDECAR_CONTENTS: &[u8] = b"opaque sidecar payload\n";
const OWNER_MANIFEST_NAME: &str = ".ast-index-owner-v1.json";

struct CacheEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl CacheEnvironment {
    fn isolated(cache_dir: &Path) -> Self {
        const KEYS: [&str; 5] = [
            "AST_INDEX_CACHE_DIR",
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
            "AST_INDEX_NO_CANONICALIZE",
            "AST_INDEX_CANONICALIZE_TIMEOUT_MS",
        ];
        let previous = KEYS
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();

        std::env::set_var("AST_INDEX_CACHE_DIR", cache_dir);
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_NO_CANONICALIZE");
        std::env::remove_var("AST_INDEX_CANONICALIZE_TIMEOUT_MS");

        Self { previous }
    }
}

impl Drop for CacheEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn djb2(value: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in value.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u64::from(byte));
    }
    format!("{hash:x}")
}

fn current_cache_key(project_root: &Path) -> String {
    djb2(&db::normalize_root_for_storage(project_root))
}

fn foreign_cache_key(keys_to_avoid: &[String]) -> String {
    ["a11ce", "b0b", "cafe", "deadbeef"]
        .into_iter()
        .find(|candidate| keys_to_avoid.iter().all(|key| key.as_str() != *candidate))
        .expect("test cache-key candidates exhausted")
        .to_owned()
}

fn create_foreign_cache(base: &Path, key: &str, project_root: &str) -> PathBuf {
    let cache_dir = base.join(key);
    fs::create_dir_all(&cache_dir).unwrap();

    let conn = Connection::open(cache_dir.join("index.db")).unwrap();
    conn.execute_batch("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('project_root', ?1)",
        params![project_root],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('fixture', 'preserved')",
        [],
    )
    .unwrap();
    drop(conn);

    fs::write(cache_dir.join("marker.txt"), MARKER_CONTENTS).unwrap();
    fs::write(cache_dir.join(SIDECAR_NAME), SIDECAR_CONTENTS).unwrap();
    cache_dir
}

fn assert_cache_intact(cache_dir: &Path) {
    assert!(cache_dir.join("index.db").is_file());
    assert_eq!(
        fs::read(cache_dir.join("marker.txt")).unwrap(),
        MARKER_CONTENTS
    );
    assert_eq!(
        fs::read(cache_dir.join(SIDECAR_NAME)).unwrap(),
        SIDECAR_CONTENTS
    );
}

fn write_owner_manifest(cache_dir: &Path, normalized_root: &str, raw_root: &str) {
    fs::write(
        cache_dir.join(OWNER_MANIFEST_NAME),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "normalized_root": normalized_root,
            "raw_root": raw_root,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn acquire_external_shared_lease(cache_base: &Path, key: &str) -> File {
    let leases = cache_base.join(".leases");
    fs::create_dir_all(&leases).unwrap();
    let lease = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(leases.join(format!("{key}.lock")))
        .unwrap();
    fs2::FileExt::lock_shared(&lease).unwrap();
    lease
}

#[test]
fn foreign_cache_migration_moves_the_whole_directory_and_respects_its_lease() {
    let temp = TempDir::new().unwrap();
    let cache_base = temp.path().join("cache");
    let project = temp.path().join("project");
    let independent_project = temp.path().join("independent-project");
    let active_foreign_project = temp.path().join("active-foreign-project");
    let aliased_project = temp.path().join("aliased-project");
    let leased_project = temp.path().join("leased-project");
    let locked_project = temp.path().join("locked-project");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&independent_project).unwrap();
    fs::create_dir_all(&active_foreign_project).unwrap();
    fs::create_dir_all(&aliased_project).unwrap();
    fs::create_dir_all(&leased_project).unwrap();
    fs::create_dir_all(&locked_project).unwrap();
    let _environment = CacheEnvironment::isolated(&cache_base);

    let current_key = current_cache_key(&project);
    let independent_key = current_cache_key(&independent_project);
    let active_foreign_key = current_cache_key(&active_foreign_project);
    let aliased_key = current_cache_key(&aliased_project);
    let leased_current_key = current_cache_key(&leased_project);
    let locked_current_key = current_cache_key(&locked_project);
    let foreign_key = foreign_cache_key(&[
        current_key.clone(),
        independent_key.clone(),
        active_foreign_key.clone(),
        aliased_key.clone(),
        leased_current_key.clone(),
        locked_current_key.clone(),
    ]);
    let leased_foreign_key = foreign_cache_key(&[
        current_key.clone(),
        independent_key.clone(),
        active_foreign_key.clone(),
        aliased_key.clone(),
        leased_current_key.clone(),
        locked_current_key.clone(),
        foreign_key.clone(),
    ]);
    let locked_foreign_key = foreign_cache_key(&[
        current_key.clone(),
        independent_key.clone(),
        active_foreign_key.clone(),
        aliased_key.clone(),
        leased_current_key.clone(),
        locked_current_key.clone(),
        foreign_key.clone(),
        leased_foreign_key.clone(),
    ]);

    let manifest_only_project = temp.path().join("manifest-only-active-project");
    fs::create_dir_all(&manifest_only_project).unwrap();
    let manifest_only_key = current_cache_key(&manifest_only_project);
    let manifest_only_lease = db::acquire_project_lease(&manifest_only_project).unwrap();
    assert!(!cache_base
        .join(&manifest_only_key)
        .join("index.db")
        .exists());
    let manifest_only_open = db::open_db_leased(&manifest_only_project).unwrap();
    assert!(cache_base
        .join(manifest_only_key)
        .join("index.db")
        .is_file());
    drop(manifest_only_open);
    drop(manifest_only_lease);

    let rebuild_active_project = temp.path().join("rebuild-active-project");
    fs::create_dir_all(&rebuild_active_project).unwrap();
    drop(db::open_db_leased(&rebuild_active_project).unwrap());
    let rebuild_key = current_cache_key(&rebuild_active_project);
    let staged_path = cache_base.join(&rebuild_key).join(".rebuild-test/index.db");
    fs::create_dir_all(staged_path.parent().unwrap()).unwrap();
    let staged = db::open_staged_db(&rebuild_active_project, &staged_path).unwrap();
    db::init_db(&staged).unwrap();
    db::seal_staged_db(staged, &staged_path).unwrap();
    let old_reader = db::open_db_leased(&rebuild_active_project).unwrap();
    old_reader
        .query_row("SELECT COUNT(*) FROM metadata", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    drop(old_reader);
    let publisher = db::acquire_index_publication_guard(&rebuild_active_project).unwrap();
    publisher.install_staged(&staged_path).unwrap();
    drop(publisher);

    // A different process can be in a write transaction while this project
    // resolves its cache. The persisted cache identity proves that the busy DB
    // is foreign, so it must not block or be considered for migration.
    drop(db::open_db_leased(&active_foreign_project).unwrap());
    let active_foreign_path = cache_base.join(&active_foreign_key).join("index.db");
    let active_foreign_lease = acquire_external_shared_lease(&cache_base, &active_foreign_key);
    let active_foreign = Connection::open(&active_foreign_path).unwrap();
    active_foreign
        .execute_batch("PRAGMA journal_mode = DELETE; BEGIN EXCLUSIVE;")
        .unwrap();
    let independent = db::open_db_leased(&independent_project).unwrap();
    assert!(
        active_foreign_path.is_file(),
        "the active foreign cache must remain in place"
    );
    assert_eq!(
        db::get_db_path(&independent_project).unwrap(),
        cache_base.join(&independent_key).join("index.db")
    );
    drop(independent);
    active_foreign.execute_batch("ROLLBACK;").unwrap();
    fs2::FileExt::unlock(&active_foreign_lease).unwrap();
    drop(active_foreign_lease);
    drop(active_foreign);

    let alternate_raw_path = aliased_project.join("..").join("aliased-project");
    assert_ne!(alternate_raw_path, aliased_project);
    assert_eq!(
        db::normalize_root_for_storage(&alternate_raw_path),
        db::normalize_root_for_storage(&aliased_project)
    );
    drop(db::open_db_leased(&alternate_raw_path).unwrap());
    drop(db::open_db_leased(&aliased_project).unwrap());

    let normalized_root = db::normalize_root_for_storage(&project);
    let source = create_foreign_cache(&cache_base, &foreign_key, &normalized_root);
    let target = cache_base.join(&current_key);

    let conn = db::open_db_leased(&project).unwrap();
    let fixture: String = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fixture, "preserved");
    assert_eq!(db::get_db_path(&project).unwrap(), target.join("index.db"));
    assert!(
        !source.exists(),
        "the foreign cache directory must disappear after migration"
    );
    assert_cache_intact(&target);
    let owner: serde_json::Value =
        serde_json::from_slice(&fs::read(target.join(OWNER_MANIFEST_NAME)).unwrap()).unwrap();
    assert_eq!(owner["normalized_root"], normalized_root);
    drop(conn);

    let leased_normalized_root = db::normalize_root_for_storage(&leased_project);
    let leased_source =
        create_foreign_cache(&cache_base, &leased_foreign_key, &leased_normalized_root);
    let leased_target = cache_base.join(&leased_current_key);
    let lease = acquire_external_shared_lease(&cache_base, &leased_foreign_key);

    let while_held = match db::open_db_leased(&leased_project) {
        Ok(_) => panic!("matching cache unexpectedly migrated while its lease was held"),
        Err(error) => error,
    };
    assert!(
        format!("{while_held:#}").contains("cache candidate")
            && format!("{while_held:#}").contains("active")
            && format!("{while_held:#}").contains("safely inspected"),
        "unexpected retryable error: {while_held:#}"
    );
    assert!(
        leased_source.is_dir(),
        "a leased source cache must not be moved: {while_held:#}"
    );
    assert_cache_intact(&leased_source);
    assert!(
        !leased_target.join("index.db").exists(),
        "a failed migration must not create a split-brain target index"
    );

    fs2::FileExt::unlock(&lease).unwrap();
    drop(lease);

    let conn = db::open_db_leased(&leased_project).unwrap();
    let fixture: String = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fixture, "preserved");
    assert_eq!(
        db::get_db_path(&leased_project).unwrap(),
        leased_target.join("index.db")
    );
    assert!(
        !leased_source.exists(),
        "the source must migrate once its external lease is released"
    );
    assert_cache_intact(&leased_target);

    let locked_normalized_root = db::normalize_root_for_storage(&locked_project);
    let locked_source =
        create_foreign_cache(&cache_base, &locked_foreign_key, &locked_normalized_root);
    let locked_target = cache_base.join(&locked_current_key);
    let locked_lease = acquire_external_shared_lease(&cache_base, &locked_foreign_key);
    let sqlite_lock = Connection::open(locked_source.join("index.db")).unwrap();
    sqlite_lock
        .execute_batch("PRAGMA journal_mode = DELETE; BEGIN EXCLUSIVE;")
        .unwrap();

    let error = match db::open_db_leased(&locked_project) {
        Ok(_) => panic!("locked cache candidate unexpectedly produced a target index"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("could not be safely inspected"),
        "unexpected locked-candidate error: {error:#}"
    );
    assert!(locked_source.join("index.db").is_file());
    assert!(!locked_target.join("index.db").exists());

    sqlite_lock.execute_batch("ROLLBACK;").unwrap();
    fs2::FileExt::unlock(&locked_lease).unwrap();
    drop(locked_lease);
    drop(sqlite_lock);
    let migrated = db::open_db_leased(&locked_project).unwrap();
    assert!(!locked_source.exists());
    drop(migrated);

    // A remounted project can keep the same raw path while resolving to a new
    // normalized root. The verified owner bridges that identity change even
    // though legacy SQLite metadata contains only the old normalized root.
    let remounted_project = temp.path().join("remounted-project");
    fs::create_dir_all(&remounted_project).unwrap();
    let remounted_raw = remounted_project.to_string_lossy().into_owned();
    let old_normalized_root = format!("{remounted_raw}-old-normalized");
    let remounted_source_key = djb2(&old_normalized_root);
    let remounted_source =
        create_foreign_cache(&cache_base, &remounted_source_key, &old_normalized_root);
    write_owner_manifest(&remounted_source, &old_normalized_root, &remounted_raw);
    let remounted = db::open_db_leased(&remounted_project).unwrap();
    let fixture: String = remounted
        .query_row(
            "SELECT value FROM metadata WHERE key = 'fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fixture, "preserved");
    assert!(!remounted_source.exists());
    drop(remounted);

    // Simulate a crash after moving an old-key directory onto the current
    // key but before rewriting its owner. Matching SQLite metadata proves
    // that this is an interrupted re-key and lets resolution heal it.
    let interrupted_parent = temp.path().join("interrupted-rekey-project");
    let interrupted_real = interrupted_parent.join("real");
    let interrupted_alias = interrupted_parent.join("alias");
    fs::create_dir_all(&interrupted_real).unwrap();
    fs::create_dir_all(&interrupted_alias).unwrap();
    let interrupted_project = interrupted_alias.join("..").join("real");
    let interrupted_normalized = db::normalize_root_for_storage(&interrupted_project);
    let interrupted_raw = interrupted_project.to_string_lossy().into_owned();
    let interrupted_old_normalized = format!("{interrupted_raw}-old-normalized");
    let interrupted_key = current_cache_key(&interrupted_project);
    assert_ne!(djb2(&interrupted_old_normalized), interrupted_key);
    assert_ne!(djb2(&interrupted_raw), interrupted_key);
    let interrupted_target =
        create_foreign_cache(&cache_base, &interrupted_key, &interrupted_old_normalized);
    write_owner_manifest(
        &interrupted_target,
        &interrupted_old_normalized,
        &interrupted_raw,
    );

    let interrupted = db::open_db_leased(&interrupted_project).unwrap();
    let fixture: String = interrupted
        .query_row(
            "SELECT value FROM metadata WHERE key = 'fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fixture, "preserved");
    let healed_owner: serde_json::Value =
        serde_json::from_slice(&fs::read(interrupted_target.join(OWNER_MANIFEST_NAME)).unwrap())
            .unwrap();
    assert_eq!(healed_owner["normalized_root"], interrupted_normalized);
    assert!(healed_owner["known_roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|root| root == &interrupted_old_normalized));
    drop(interrupted);

    // Owner overlap alone is insufficient: a direct target whose database
    // metadata belongs elsewhere must stay untouched and fail closed.
    let interrupted_conflict_parent = temp.path().join("interrupted-rekey-conflict");
    let interrupted_conflict_real = interrupted_conflict_parent.join("real");
    let interrupted_conflict_alias = interrupted_conflict_parent.join("alias");
    fs::create_dir_all(&interrupted_conflict_real).unwrap();
    fs::create_dir_all(&interrupted_conflict_alias).unwrap();
    let interrupted_conflict_project = interrupted_conflict_alias.join("..").join("real");
    let interrupted_conflict_raw = interrupted_conflict_project.to_string_lossy().into_owned();
    let interrupted_conflict_old = format!("{interrupted_conflict_raw}-old-normalized");
    let interrupted_conflict_key = current_cache_key(&interrupted_conflict_project);
    assert_ne!(djb2(&interrupted_conflict_old), interrupted_conflict_key);
    assert_ne!(djb2(&interrupted_conflict_raw), interrupted_conflict_key);
    let interrupted_conflict_target = create_foreign_cache(
        &cache_base,
        &interrupted_conflict_key,
        "/unrelated/direct-target",
    );
    write_owner_manifest(
        &interrupted_conflict_target,
        &interrupted_conflict_old,
        &interrupted_conflict_raw,
    );
    let interrupted_conflict = match db::open_db_leased(&interrupted_conflict_project) {
        Ok(_) => panic!("conflicting direct target was re-keyed"),
        Err(error) => error,
    };
    assert!(
        format!("{interrupted_conflict:#}").contains("project_root metadata"),
        "unexpected interrupted-rekey conflict: {interrupted_conflict:#}"
    );
    let unchanged_owner: serde_json::Value = serde_json::from_slice(
        &fs::read(interrupted_conflict_target.join(OWNER_MANIFEST_NAME)).unwrap(),
    )
    .unwrap();
    assert_eq!(unchanged_owner["normalized_root"], interrupted_conflict_old);

    // An overlapping manifest is a matching candidate, so readable SQLite
    // metadata must belong to that owner before migration is allowed.
    let conflict_project = temp.path().join("manifest-conflict-project");
    fs::create_dir_all(&conflict_project).unwrap();
    let conflict_raw = conflict_project.to_string_lossy().into_owned();
    let conflict_old_normalized = format!("{conflict_raw}-old-normalized");
    let conflict_source_key = djb2(&conflict_old_normalized);
    let conflict_source =
        create_foreign_cache(&cache_base, &conflict_source_key, "/unrelated/project");
    write_owner_manifest(&conflict_source, &conflict_old_normalized, &conflict_raw);

    let conflict = match db::open_db_leased(&conflict_project) {
        Ok(_) => panic!("conflicting cache owner manifest was trusted"),
        Err(error) => error,
    };
    assert!(
        format!("{conflict:#}").contains("manifest conflicts"),
        "unexpected manifest conflict error: {conflict:#}"
    );
    assert!(conflict_source.join("index.db").is_file());
    assert!(
        !cache_base
            .join(current_cache_key(&conflict_project))
            .join("index.db")
            .exists(),
        "manifest conflict must not create a split-brain target"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        // Root discovery can retain a process-local lease while a VFS alias
        // starts resolving elsewhere. Reuse the leased identity for this
        // operation; after both connections close, the next open may migrate.
        let first_real = temp.path().join("leased-remount-first");
        let second_real = temp.path().join("leased-remount-second");
        let first_mount = temp.path().join("leased-remount-link-a");
        let second_mount = temp.path().join("leased-remount-link-b");
        fs::create_dir_all(&first_real).unwrap();
        fs::create_dir_all(&second_real).unwrap();
        symlink(&first_real, &first_mount).unwrap();
        symlink(&first_real, &second_mount).unwrap();
        let second_mount_raw = second_mount.to_string_lossy().into_owned();
        let first_normalized = db::normalize_root_for_storage(&first_mount);

        let first_key = current_cache_key(&first_mount);
        let first = db::open_db_leased(&first_mount).unwrap();
        first
            .execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES ('lease_fixture', 'pinned')",
                [],
            )
            .unwrap();
        let same_target_alias = db::open_db_leased(&second_mount).unwrap();
        let owner_after_alias: serde_json::Value = serde_json::from_slice(
            &fs::read(cache_base.join(&first_key).join(OWNER_MANIFEST_NAME)).unwrap(),
        )
        .unwrap();
        assert!(owner_after_alias["known_roots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|root| root.as_str() == Some(second_mount_raw.as_str())));

        fs::remove_file(&second_mount).unwrap();
        symlink(&second_real, &second_mount).unwrap();
        let second_key = current_cache_key(&second_mount);
        assert_ne!(first_key, second_key);

        let second = db::open_db_leased(&second_mount).unwrap();
        let pinned: String = second
            .query_row(
                "SELECT value FROM metadata WHERE key = 'lease_fixture'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pinned, "pinned");
        let pinned_root: String = second
            .query_row(
                "SELECT value FROM metadata WHERE key = 'project_root'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pinned_root, first_normalized);
        assert!(!cache_base.join(&second_key).join("index.db").exists());

        drop(second);
        drop(same_target_alias);
        drop(first);
        let migrated = db::open_db_leased(&second_mount).unwrap();
        let preserved: String = migrated
            .query_row(
                "SELECT value FROM metadata WHERE key = 'lease_fixture'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, "pinned");
        assert!(!cache_base.join(first_key).exists());
        assert!(cache_base.join(second_key).join("index.db").is_file());
        drop(migrated);
    }

    // Historical raw hashes for relative roots are process-context
    // ambiguous. A cache keyed and tagged only as `.` must not be stolen by
    // the current working directory's absolute project identity.
    let relative_root = Path::new(".");
    let relative_raw_key = djb2(".");
    let relative_current_key = current_cache_key(relative_root);
    assert_ne!(relative_raw_key, relative_current_key);
    let ambiguous_relative = create_foreign_cache(
        &cache_base,
        &relative_raw_key,
        relative_root.to_str().unwrap(),
    );
    let relative = db::open_db_leased(relative_root).unwrap();
    assert!(ambiguous_relative.join("index.db").is_file());
    assert!(cache_base
        .join(relative_current_key)
        .join("index.db")
        .is_file());
    let relative_fixture = relative.query_row(
        "SELECT value FROM metadata WHERE key = 'fixture'",
        [],
        |row| row.get::<_, String>(0),
    );
    assert!(relative_fixture.is_err());
}
