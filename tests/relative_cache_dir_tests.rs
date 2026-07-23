use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;

use ast_index::db;
use tempfile::TempDir;

struct ProcessEnvironment {
    cwd: PathBuf,
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl ProcessEnvironment {
    fn relative_cache() -> Self {
        let keys = [
            "AST_INDEX_CACHE_DIR",
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
            "AST_INDEX_NO_CANONICALIZE",
            "AST_INDEX_CANONICALIZE_TIMEOUT_MS",
        ];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_var("AST_INDEX_CACHE_DIR", "cache");
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_NO_CANONICALIZE");
        std::env::remove_var("AST_INDEX_CANONICALIZE_TIMEOUT_MS");
        Self { cwd, previous }
    }
}

impl Drop for ProcessEnvironment {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.cwd).unwrap();
        for (key, value) in self.previous.drain(..) {
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
        hash = hash.wrapping_mul(33).wrapping_add(u64::from(byte));
    }
    format!("{hash:x}")
}

#[test]
fn relative_cache_override_is_bound_to_the_resolving_cwd() {
    let tmp = TempDir::new().unwrap();
    let first_cwd = tmp.path().join("first");
    let second_cwd = tmp.path().join("second");
    let project = tmp.path().join("project");
    fs::create_dir_all(&first_cwd).unwrap();
    fs::create_dir_all(&second_cwd).unwrap();
    fs::create_dir_all(&project).unwrap();
    let _environment = ProcessEnvironment::relative_cache();
    let key = hash(&db::normalize_root_for_storage(&project));

    std::env::set_current_dir(&first_cwd).unwrap();
    let first_lease = db::acquire_project_lease(&project).unwrap();
    let first_lock = first_cwd.join("cache/.leases").join(format!("{key}.lock"));
    assert!(first_lock.is_file());

    std::env::set_current_dir(&second_cwd).unwrap();
    let second_lease = db::acquire_project_lease(&project).unwrap();
    let second_lock = second_cwd.join("cache/.leases").join(format!("{key}.lock"));
    assert!(second_lock.is_file(), "second cwd needs its own lease file");

    let probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&second_lock)
        .unwrap();
    assert!(
        fs2::FileExt::try_lock_exclusive(&probe).is_err(),
        "second cache must be protected by its own shared lease"
    );

    drop(second_lease);
    fs2::FileExt::try_lock_exclusive(&probe).unwrap();
    fs2::FileExt::unlock(&probe).unwrap();
    drop(first_lease);
}
