//! End-to-end regression for CLI-triggered stale-cache collection.
//!
//! The subprocesses use an isolated `AST_INDEX_CACHE_DIR`; this test never
//! changes the test runner's environment or the developer's real cache.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use ast_index::db;
use tempfile::TempDir;

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ast-index"))
}

fn ast_index_command(project: &Path, cache: &Path) -> Command {
    let mut command = Command::new(binary());
    command
        .current_dir(project)
        .env("AST_INDEX_CACHE_DIR", cache)
        // Do not let an outer test/developer environment redirect this E2E
        // away from the isolated multi-project cache layout.
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_DISABLE_GC")
        .env_remove("AST_INDEX_MAX_FILES");
    command
}

fn run(project: &Path, cache: &Path, args: &[&str], disable_gc: bool) -> Output {
    let mut command = ast_index_command(project, cache);
    command.args(args);
    if disable_gc {
        command.env("AST_INDEX_DISABLE_GC", "1");
    }
    command.output().expect("ast-index subprocess should start")
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn is_cache_key(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 16
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn project_cache_keys(cache: &Path) -> Vec<String> {
    let mut keys = fs::read_dir(cache)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (is_cache_key(&name) && entry.path().join("index.db").is_file()).then_some(name)
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn create_stale_cache(cache: &Path, key: &str) -> PathBuf {
    let dir = cache.join(key);
    fs::create_dir_all(&dir).unwrap();
    let database = dir.join("index.db");
    fs::write(&database, b"foreign stale cache").unwrap();
    let stale_mtime = SystemTime::now()
        .checked_sub(db::STALE_CACHE_MAX_AGE + Duration::from_secs(60))
        .unwrap();
    OpenOptions::new()
        .write(true)
        .open(&database)
        .unwrap()
        .set_modified(stale_mtime)
        .unwrap();
    dir
}

#[test]
fn successful_update_dispatch_honors_gc_guard_then_collects_stale_cache() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let cache = temp.path().join("cache");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"gc-e2e\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn indexed() {}\n").unwrap();

    let rebuild = run(&project, &cache, &["rebuild"], false);
    assert_success("rebuild", &rebuild);

    let current_keys = project_cache_keys(&cache);
    assert_eq!(
        current_keys.len(),
        1,
        "rebuild should create exactly one current cache, got {current_keys:?}"
    );
    let current_key = &current_keys[0];
    let stale_key = ["deadbeefdeadbeef", "cafebabecafebabe"]
        .into_iter()
        .find(|candidate| *candidate != current_key)
        .unwrap();
    assert!(is_cache_key(stale_key));
    assert_ne!(stale_key, current_key);
    let stale_cache = create_stale_cache(&cache, stale_key);

    let guarded_update = run(&project, &cache, &["update"], true);
    assert_success("guarded update", &guarded_update);
    assert!(
        stale_cache.join("index.db").is_file(),
        "AST_INDEX_DISABLE_GC=1 must leave the stale cache untouched"
    );

    let collecting_update = run(&project, &cache, &["update"], false);
    assert_success("collecting update", &collecting_update);
    assert!(
        !stale_cache.exists(),
        "an unguarded successful update must collect the stale cache"
    );
    assert!(
        cache.join(current_key).join("index.db").is_file(),
        "GC must preserve the cache of the project being updated"
    );
    assert!(
        String::from_utf8_lossy(&collecting_update.stderr).contains("removed 1 stale index cache"),
        "CLI dispatch should report the collected cache; stderr:\n{}",
        String::from_utf8_lossy(&collecting_update.stderr)
    );
}
