//! Regression tests for the three memory-peak guards added to rebuild:
//!
//!   * signature truncation at DB insert (chokepoint replaces 220 callsites)
//!   * `AST_INDEX_MAX_FILE_SIZE` skip on oversized files
//!   * `AST_INDEX_MAX_FILES` walker cap with helpful error and metadata bypass

use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard};

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

/// Tests in this file mutate process-wide env vars
/// (AST_INDEX_MAX_FILES, AST_INDEX_MAX_FILE_SIZE). `cargo test` runs them in
/// parallel by default, so without serialization they race and flake. Acquire
/// this lock at the top of every test to force one-at-a-time execution
/// regardless of test thread count.
static ENV_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Reset the indexer's env knobs so tests don't bleed into each other.
struct EnvScope {
    keys: Vec<(&'static str, Option<String>)>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvScope {
    fn new(keys: &[&'static str]) -> Self {
        // Recover from a poisoned lock (a previous panicked test) — we only
        // care about serialization, not about preserving any shared state.
        let guard = ENV_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let saved = keys
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect::<Vec<_>>();
        for k in keys {
            std::env::remove_var(k);
        }
        Self {
            keys: saved,
            _guard: guard,
        }
    }

    fn set(&self, k: &str, v: &str) {
        std::env::set_var(k, v);
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        for (k, v) in &self.keys {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
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

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn signature_truncated_at_db_insert() {
    // A single Rust file with many top-level fn declarations, each on a
    // *very long* single line. After the fix, every stored signature is
    // ≤ 503 bytes (500 + "...") regardless of source line length.
    let env = EnvScope::new(&["AST_INDEX_MAX_FILE_SIZE", "AST_INDEX_MAX_FILES"]);
    env.set("AST_INDEX_MAX_FILE_SIZE", "10_000_000"); // don't trip size cap

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let mut source = String::new();
    // 50 functions, each on a 2 KB line.
    for i in 0..50 {
        let filler = "x".repeat(2_000);
        source.push_str(&format!("pub fn f{i}() {{ let _ = \"{filler}\"; }}\n"));
    }
    write(&root.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    write(&root.join("src/lib.rs"), &source);

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    let mut stmt = conn.prepare("SELECT signature FROM symbols").unwrap();
    let sigs: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(!sigs.is_empty(), "expected indexed symbols");
    for s in &sigs {
        assert!(
            s.len() <= 503,
            "signature exceeded cap: {} bytes",
            s.len()
        );
    }
}

#[test]
fn file_size_cap_skips_parsing_but_keeps_file_row() {
    let env = EnvScope::new(&["AST_INDEX_MAX_FILE_SIZE", "AST_INDEX_MAX_FILES"]);
    env.set("AST_INDEX_MAX_FILE_SIZE", "1024"); // 1 KB

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    // Source file > 1 KB cap — must be recorded but not parsed.
    let big = "fn huge() {}\n".repeat(200); // ~2.6 KB
    write(&root.join("src/big.rs"), &big);
    // Small file < cap — must be parsed normally.
    write(&root.join("src/small.rs"), "pub fn small_one() {}\n");

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    let big_symbols: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols s JOIN files f ON s.file_id = f.id \
             WHERE f.path LIKE '%big.rs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(big_symbols, 0, "big.rs must not contribute symbols");

    let small_symbols: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols s JOIN files f ON s.file_id = f.id \
             WHERE f.path LIKE '%small.rs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(small_symbols > 0, "small.rs must be parsed normally");

    // file row for the oversized file must still exist (so `update` later
    // notices on-disk changes).
    let big_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path LIKE '%big.rs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(big_present, 1);
}

#[test]
fn walker_cap_aborts_with_actionable_error() {
    let env = EnvScope::new(&["AST_INDEX_MAX_FILES"]);
    env.set("AST_INDEX_MAX_FILES", "5");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    // 15 source files — well above the cap of 5.
    for i in 0..15 {
        write(
            &root.join(format!("src/file_{i}.rs")),
            "pub fn placeholder() {}\n",
        );
    }

    let mut conn = open_fresh_db(root);
    let err = indexer::index_directory(&mut conn, root, false, false)
        .err()
        .expect("walker cap must abort");
    let msg = err.to_string();
    assert!(
        msg.contains("walker stopped"),
        "missing abort message: {msg}"
    );
    assert!(msg.contains("--force"), "must mention --force: {msg}");
    assert!(msg.contains("--max-files"), "must mention --max-files: {msg}");
    assert!(
        msg.contains("AST_INDEX_MAX_FILES"),
        "must mention env override: {msg}"
    );
}

#[test]
fn walker_cap_bypassed_via_metadata_flag() {
    let env = EnvScope::new(&["AST_INDEX_MAX_FILES"]);
    env.set("AST_INDEX_MAX_FILES", "5");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    for i in 0..15 {
        write(
            &root.join(format!("src/file_{i}.rs")),
            "pub fn placeholder() {}\n",
        );
    }

    let mut conn = open_fresh_db(root);
    // Simulate prior `rebuild --force --remember`.
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('bypass_size_check', '1')",
        [],
    )
    .unwrap();

    let result = indexer::index_directory(&mut conn, root, false, false);
    assert!(
        result.is_ok(),
        "bypass_size_check=1 must override the cap, got: {:?}",
        result.err()
    );
}

#[test]
fn walker_cap_disabled_when_env_is_zero() {
    let env = EnvScope::new(&["AST_INDEX_MAX_FILES"]);
    env.set("AST_INDEX_MAX_FILES", "0");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    for i in 0..50 {
        write(
            &root.join(format!("src/file_{i}.rs")),
            "pub fn placeholder() {}\n",
        );
    }

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false)
        .expect("cap = 0 must disable the limit entirely");
}
