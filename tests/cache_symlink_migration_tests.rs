#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;

use ast_index::db;
use rusqlite::Connection;
use tempfile::TempDir;

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn isolated_cache(path: &std::path::Path) -> Self {
        let keys = [
            "AST_INDEX_CACHE_DIR",
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
            "AST_INDEX_NO_CANONICALIZE",
            "AST_INDEX_CANONICALIZE_TIMEOUT_MS",
        ];
        let saved = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("AST_INDEX_CACHE_DIR", path);
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_NO_CANONICALIZE");
        std::env::remove_var("AST_INDEX_CANONICALIZE_TIMEOUT_MS");
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn hash(value: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in value.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    format!("{hash:x}")
}

#[test]
fn raw_hash_migration_ignores_symlinked_cache_source() {
    let tmp = TempDir::new().unwrap();
    let real_project = tmp.path().join("real-project");
    let alias_project = tmp.path().join("project-alias");
    let cache = tmp.path().join("cache");
    let victim = tmp.path().join("victim");
    fs::create_dir_all(&real_project).unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("index.db"), "do not move").unwrap();
    symlink(&real_project, &alias_project).unwrap();

    let raw_key = hash(alias_project.to_string_lossy().as_ref());
    let canonical = fs::canonicalize(&alias_project).unwrap();
    let canonical_key = hash(canonical.to_string_lossy().as_ref());
    assert_ne!(raw_key, canonical_key);
    symlink(&victim, cache.join(&raw_key)).unwrap();

    let _env = EnvGuard::isolated_cache(&cache);
    let db_path = db::get_db_path(&alias_project).unwrap();

    assert_eq!(db_path, cache.join(canonical_key).join("index.db"));
    assert!(
        fs::symlink_metadata(cache.join(raw_key))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the raw-hash symlink must not be renamed as a cache directory"
    );
    assert_eq!(
        fs::read_to_string(victim.join("index.db")).unwrap(),
        "do not move"
    );

    let second_project = tmp.path().join("second-project");
    let outside_target = tmp.path().join("outside-target");
    fs::create_dir_all(&second_project).unwrap();
    fs::create_dir_all(&outside_target).unwrap();
    let second_key = hash(&db::normalize_root_for_storage(&second_project));
    symlink(&outside_target, cache.join(second_key)).unwrap();

    let error = db::get_db_path(&second_project).unwrap_err();
    assert!(
        format!("{error:#}").contains("not a real directory"),
        "unexpected symlink-target error: {error:#}"
    );
    assert!(
        fs::read_dir(&outside_target).unwrap().next().is_none(),
        "cache resolution must not create files through a target symlink"
    );

    let third_project = tmp.path().join("third-project");
    fs::create_dir_all(&third_project).unwrap();
    let third_key = hash(&db::normalize_root_for_storage(&third_project));
    let third_cache = cache.join(third_key);
    fs::create_dir_all(&third_cache).unwrap();
    let outside_db = tmp.path().join("outside.db");
    let outside_conn = Connection::open(&outside_db).unwrap();
    outside_conn
        .execute("CREATE TABLE sentinel (value TEXT NOT NULL)", [])
        .unwrap();
    drop(outside_conn);
    let original_bytes = fs::read(&outside_db).unwrap();
    symlink(&outside_db, third_cache.join("index.db")).unwrap();

    let path_error = db::get_db_path(&third_project).unwrap_err();
    assert!(
        format!("{path_error:#}").contains("not a regular file"),
        "unexpected index.db symlink error: {path_error:#}"
    );
    let open_error = match db::open_db_leased(&third_project) {
        Ok(_) => panic!("leased open unexpectedly followed an index.db symlink"),
        Err(error) => error,
    };
    assert!(
        format!("{open_error:#}").contains("not a regular file"),
        "unexpected leased-open symlink error: {open_error:#}"
    );
    assert_eq!(
        fs::read(&outside_db).unwrap(),
        original_bytes,
        "cache resolution/open must not follow a preplanted index.db symlink"
    );

    let fourth_project = tmp.path().join("fourth-project");
    fs::create_dir_all(&fourth_project).unwrap();
    let fourth_key = hash(&db::normalize_root_for_storage(&fourth_project));
    let fourth_cache = cache.join(fourth_key);
    fs::create_dir_all(&fourth_cache).unwrap();
    let outside_wal = tmp.path().join("outside-wal");
    fs::write(&outside_wal, b"outside wal").unwrap();
    symlink(&outside_wal, fourth_cache.join("index.db-wal")).unwrap();

    let wal_error = db::get_db_path(&fourth_project).unwrap_err();
    assert!(
        format!("{wal_error:#}").contains("not a regular file"),
        "unexpected WAL symlink error: {wal_error:#}"
    );
    assert_eq!(fs::read(&outside_wal).unwrap(), b"outside wal");
}
