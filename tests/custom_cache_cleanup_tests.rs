use std::ffi::OsString;
use std::fs;
use std::path::Path;

use ast_index::db;
use tempfile::TempDir;

struct CacheEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl CacheEnvironment {
    fn set(path: &Path) -> Self {
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
        std::env::set_var("AST_INDEX_CACHE_DIR", path);
        std::env::remove_var("AST_INDEX_DB_PATH");
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_NO_CANONICALIZE");
        std::env::remove_var("AST_INDEX_CANONICALIZE_TIMEOUT_MS");
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

fn hash(value: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in value.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    format!("{hash:x}")
}

#[test]
fn custom_cache_overrides_never_touch_legacy_locations() {
    let tmp = TempDir::new().unwrap();
    {
        let custom_cache = tmp.path().join("named-cache").join("kotlin-index");
        fs::create_dir_all(&custom_cache).unwrap();
        fs::write(custom_cache.join("current-data"), "keep").unwrap();
        let _environment = CacheEnvironment::set(&custom_cache);

        db::cleanup_legacy_cache();

        assert_eq!(
            fs::read_to_string(custom_cache.join("current-data")).unwrap(),
            "keep"
        );
    }

    // A custom target must not infer a historical source from its parent.
    // That sibling is outside the custom cache's lock namespace, so moving
    // from it would be an uncoordinated cross-layout race.
    let layout = tmp.path().join("migration-layout");
    let custom_cache = layout.join("ast-index");
    let legacy_cache = layout.join("kotlin-index");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let raw_key = hash(project.to_string_lossy().as_ref());
    let legacy_project = legacy_cache.join(&raw_key);
    fs::create_dir_all(&legacy_project).unwrap();
    fs::write(legacy_project.join("index.db"), b"legacy bytes").unwrap();

    let _environment = CacheEnvironment::set(&custom_cache);
    let lease = db::migrate_legacy_project_with_lease(&project).unwrap();
    let normalized_key = hash(
        fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .as_ref(),
    );

    assert_eq!(
        fs::read(legacy_project.join("index.db")).unwrap(),
        b"legacy bytes"
    );
    assert!(!custom_cache.join(normalized_key).join("index.db").exists());
    drop(lease);
}
