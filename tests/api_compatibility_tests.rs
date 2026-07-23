use std::ffi::OsString;
use std::fs::File;
use std::path::Path;

use ast_index::commands::{self, PathResolver};
use ast_index::db;
use rusqlite::Connection;
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
fn historical_public_function_signatures_remain_source_compatible() {
    let _: fn(&Path) -> anyhow::Result<Connection> = db::open_db;
    let _: fn(&Path, &Connection) -> PathResolver = PathResolver::from_conn;
    let _: fn(&Path) -> bool = commands::is_no_ignore_enabled;
    let _: fn(&Path) -> bool = commands::is_experimental_fast_rebuild_enabled;
    let _: fn(&Path) -> anyhow::Result<File> = db::acquire_rebuild_lock;

    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let _override = DbOverride::set(&tmp.path().join("compat.sqlite"));

    let conn: Connection = db::open_db(&project).unwrap();
    assert!(
        conn.close().is_ok(),
        "open_db must return concrete Connection"
    );
}
