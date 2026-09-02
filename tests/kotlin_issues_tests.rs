//! End-to-end regressions for Kotlin parser issues #53, #56, and #57.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn environment_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct DbOverride {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl DbOverride {
    fn set(path: &Path) -> Self {
        let keys = [
            "AST_INDEX_DB_PATH",
            "KOTLIN_INDEX_DB_PATH",
            "AST_INDEX_CACHE_DIR",
        ];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("AST_INDEX_DB_PATH", path);
        std::env::remove_var("KOTLIN_INDEX_DB_PATH");
        std::env::remove_var("AST_INDEX_CACHE_DIR");
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

fn open_fresh_db(project_root: &Path) -> Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

#[test]
fn suspend_lambda_file_keeps_all_declarations_and_drops_local_properties() {
    let _environment_lock = environment_lock();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let _db_override = DbOverride::set(&root.join("test-index.db"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/T3.kt"),
        r#"package t

interface IThing2 : IFeature {
    suspend fun isEnabled(): Boolean

    class Impl : IThing2 {
        override suspend fun isEnabled(): Boolean {
            val default = suspend { true }
            return default()
        }
    }
}

interface LaterFeature
class LaterImpl : LaterFeature
"#,
    )
    .unwrap();

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    for expected in ["IThing2", "Impl", "LaterFeature", "LaterImpl"] {
        let symbols = db::find_symbols_by_name(&conn, expected, None, 10).unwrap();
        assert!(
            symbols.iter().any(|symbol| symbol.name == expected),
            "missing {expected}: {symbols:?}"
        );
    }

    let file_symbols = db::get_file_symbols(&conn, "src/T3.kt").unwrap();
    assert_eq!(
        file_symbols
            .iter()
            .filter(|symbol| symbol.name == "isEnabled")
            .count(),
        2
    );
    assert!(!file_symbols.iter().any(|symbol| symbol.name == "default"));
}

#[test]
fn usages_keep_in_file_hits_and_ignore_non_code_text() {
    let _environment_lock = environment_lock();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let _db_override = DbOverride::set(&root.join("test-index.db"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/Engine.kt"),
        r#"package demo

class Engine(val id: String)

fun buildDefaultEngine(): Engine {
    return Engine(id = "default")
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/Garage.kt"),
        r#"package demo

fun openGarage() {
    val engine = Engine(id = "external Engine text") // Engine comment
    val raw = """Engine raw text"""
    /** Engine KDoc text */
    println("created ${Engine(id = "interpolated")}")
}
"#,
    )
    .unwrap();

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    let refs = db::find_references(&conn, "Engine", 50).unwrap();
    let hits: Vec<(String, i64)> = refs
        .iter()
        .map(|reference| (reference.path.clone(), reference.line))
        .collect();
    assert_eq!(
        hits,
        vec![
            ("src/Engine.kt".to_string(), 5),
            ("src/Engine.kt".to_string(), 6),
            ("src/Garage.kt".to_string(), 4),
            ("src/Garage.kt".to_string(), 7),
        ]
    );
    assert!(refs
        .iter()
        .find(|reference| reference.line == 7)
        .is_some_and(|reference| reference
            .context
            .as_deref()
            .is_some_and(|context| context.contains("interpolated"))));
}
