use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use ast_index::db;
use tempfile::TempDir;

struct ProcessState {
    cwd: PathBuf,
    variables: Vec<(&'static str, Option<OsString>)>,
}

impl ProcessState {
    fn capture() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap(),
            variables: ["AST_INDEX_DB_PATH", "KOTLIN_INDEX_DB_PATH"]
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        }
    }
}

impl Drop for ProcessState {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.cwd).unwrap();
        for (key, value) in self.variables.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn rebuild_swap_resolves_relative_override_before_cwd_and_env_change() {
    let _state = ProcessState::capture();
    let temp = TempDir::new().unwrap();
    let start_dir = temp.path().join("start");
    let later_dir = temp.path().join("later");
    let project = temp.path().join("project");
    fs::create_dir_all(start_dir.join("cache")).unwrap();
    fs::create_dir_all(later_dir.join("other-cache")).unwrap();
    fs::create_dir(&project).unwrap();

    let original_db = start_dir.join("cache/index.db");
    fs::write(&original_db, b"original database").unwrap();
    std::env::set_current_dir(&start_dir).unwrap();
    std::env::set_var("AST_INDEX_DB_PATH", Path::new("cache/index.db"));
    std::env::remove_var("KOTLIN_INDEX_DB_PATH");

    let swap = db::RebuildSwap::begin(&project).unwrap();
    assert!(!original_db.exists());
    assert!(start_dir.join("cache/index.db.swap").is_file());

    std::env::set_current_dir(&later_dir).unwrap();
    std::env::set_var("AST_INDEX_DB_PATH", Path::new("other-cache/index.db"));
    drop(swap);

    assert_eq!(fs::read(&original_db).unwrap(), b"original database");
    assert!(!start_dir.join("cache/index.db.swap").exists());
    assert!(!later_dir.join("cache/index.db").exists());
}
