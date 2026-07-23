use std::ffi::OsString;
use std::fs;
use std::path::Path;

use ast_index::db;
use tempfile::TempDir;

struct DbOverride {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl DbOverride {
    fn set(path: &Path) -> Self {
        let keys = ["AST_INDEX_DB_PATH", "KOTLIN_INDEX_DB_PATH"];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("AST_INDEX_DB_PATH", path);
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        Self { previous }
    }

    fn switch_to(&self, path: &Path) {
        std::env::set_var("AST_INDEX_DB_PATH", path);
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

#[test]
fn rebuild_swap_restores_the_path_resolved_at_begin() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    let original_db = tmp.path().join("original.db");
    let other_db = tmp.path().join("other.db");
    fs::create_dir(&root).unwrap();
    fs::write(&original_db, "original").unwrap();
    fs::write(&other_db, "other").unwrap();
    let env = DbOverride::set(&original_db);

    let swap = db::RebuildSwap::begin(&root).unwrap();
    assert!(!original_db.exists());
    assert!(original_db.with_extension("db.swap").exists());

    env.switch_to(&other_db);
    drop(swap);

    assert_eq!(fs::read_to_string(&original_db).unwrap(), "original");
    assert_eq!(fs::read_to_string(&other_db).unwrap(), "other");
    assert!(!original_db.with_extension("db.swap").exists());
}
