//! Regression: when `ast-index rebuild` aborts mid-flight (walker cap,
//! IO error, anything that propagates Err), the previous valid index must
//! survive intact instead of being replaced with an empty DB.
//!
//! Pre-fix behaviour: `cmd_rebuild` called `db::delete_db` *before* the
//! walker ran. If the walker then aborted on the file-count cap, the user
//! was left with an empty DB.
//!
//! Post-fix behaviour: `cmd_rebuild` builds a private staged generation while
//! the previous DB remains live, then publishes it through a short crash-safe
//! handoff. Any pre-publication error simply drops the staged generation.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{LazyLock, Mutex, MutexGuard};

use tempfile::TempDir;

static ENV_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn lock() -> MutexGuard<'static, ()> {
    ENV_SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ast-index"))
}

fn count_files_in_db(db_path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap_or(0)
}

fn count_symbols_in_db(db_path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .unwrap_or(0)
}

#[test]
fn cap_aborted_rebuild_restores_previous_index() {
    let _g = lock();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("index.db");

    write(
        &root.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0\"\n",
    );
    write(&root.join("src/lib.rs"), "pub fn one() {}\n");

    // First rebuild: produces a valid index with 1 symbol.
    let out = Command::new(binary())
        .current_dir(root)
        .args(["rebuild"])
        .env("AST_INDEX_DB_PATH", &db_path)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "first rebuild failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let baseline_files = count_files_in_db(&db_path);
    let baseline_symbols = count_symbols_in_db(&db_path);
    assert!(baseline_files >= 1, "expected baseline files");
    assert!(baseline_symbols >= 1, "expected baseline symbols");

    // Pile up more files than the cap so the next rebuild aborts.
    for i in 0..30 {
        write(
            &root.join(format!("src/extra_{i}.rs")),
            "pub fn extra() {}\n",
        );
    }

    // Second rebuild: hits the cap and must abort.
    let out = Command::new(binary())
        .current_dir(root)
        .args(["rebuild"])
        .env("AST_INDEX_DB_PATH", &db_path)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("AST_INDEX_MAX_FILES", "3")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "rebuild was expected to abort on cap, but succeeded"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("walker stopped"),
        "expected cap-abort message, got: {stderr}"
    );

    // Critical assertion: the previous index is still there. Counts must
    // match the baseline, not be 0.
    let restored_files = count_files_in_db(&db_path);
    let restored_symbols = count_symbols_in_db(&db_path);
    assert_eq!(
        restored_files, baseline_files,
        "files count drifted after aborted rebuild — index was lost"
    );
    assert_eq!(
        restored_symbols, baseline_symbols,
        "symbols count drifted after aborted rebuild — index was lost"
    );

    // And no publication leftovers on disk.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let swap = db_path.with_extension(format!("db.swap{}", suffix));
        assert!(
            !swap.exists(),
            "stale swap file left behind: {}",
            swap.display()
        );
    }
    for extension in [
        "db.swap-pending",
        "db.publish-state-v1",
        "db.publish-commit-v1",
    ] {
        assert!(!db_path.with_extension(extension).exists());
    }
    let staged = fs::read_dir(db_path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with(".rebuild-"));
    assert!(!staged, "staged rebuild directory remains after failure");
}

#[test]
fn successful_rebuild_cleans_up_swap() {
    let _g = lock();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("index.db");

    write(
        &root.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0\"\n",
    );
    write(&root.join("src/lib.rs"), "pub fn one() {}\n");

    let out = Command::new(binary())
        .current_dir(root)
        .args(["rebuild"])
        .env("AST_INDEX_DB_PATH", &db_path)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap();
    assert!(out.status.success());

    // Run again to make sure the second rebuild also leaves no swap behind.
    let out = Command::new(binary())
        .current_dir(root)
        .args(["rebuild"])
        .env("AST_INDEX_DB_PATH", &db_path)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap();
    assert!(out.status.success());

    for suffix in ["", "-wal", "-shm", "-journal"] {
        let swap = db_path.with_extension(format!("db.swap{}", suffix));
        assert!(!swap.exists(), "swap not cleaned up: {}", swap.display());
    }
    for extension in [
        "db.swap-pending",
        "db.publish-state-v1",
        "db.publish-commit-v1",
    ] {
        assert!(!db_path.with_extension(extension).exists());
    }
}
