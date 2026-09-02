use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct State {
    requested_generation: u64,
    completed_generation: u64,
    successful_cycles: u64,
    #[serde(default)]
    worker_launches: u64,
    last_error: Option<String>,
}

fn run(root: &Path, db_path: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ast-index"));
    command
        .current_dir(root)
        .env("AST_INDEX_DB_PATH", db_path)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_CACHE_DIR")
        .args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn project(temp: &TempDir, name: &str) -> (PathBuf, PathBuf) {
    let root = temp.path().join(name);
    let db_path = temp.path().join(format!("{name}.db"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname=\"{name}\"\nversion=\"0.1.0\"\n"),
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn initial() {}\n").unwrap();
    let rebuild = run(&root, &db_path, &["rebuild"], &[]);
    assert!(
        rebuild.status.success(),
        "rebuild failed: {}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    (root, db_path)
}

fn state_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("db.update-state-v1.json")
}

fn read_state(db_path: &Path) -> State {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = fs::read(state_path(db_path)) {
            if let Ok(state) = serde_json::from_slice(&bytes) {
                return state;
            }
        }
        assert!(
            Instant::now() < deadline,
            "coordinator state was not readable"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn expire_worker_claim(db_path: &Path) {
    let path = state_path(db_path);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["worker_claimed_at_ms"] = serde_json::json!(0);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn queue(root: &Path, db: &Path, debounce_ms: u64, envs: &[(&str, &Path)]) {
    let output = run(
        root,
        db,
        &[
            "update",
            "--background",
            "--debounce-ms",
            &debounce_ms.to_string(),
        ],
        envs,
    );
    assert!(
        output.status.success(),
        "queue failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_reader(root: &Path, db: &Path) -> Output {
    run(root, db, &["stats"], &[])
}

#[test]
fn update_help_exposes_background_coordinator_flags() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let db = temp.path().join("unused.db");
    let output = run(root, &db, &["update", "--help"], &[]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--background"));
    assert!(stdout.contains("--debounce-ms"));
    assert!(!stdout.contains("--coordinator-worker"));
    assert!(!stdout.contains("--coordinator-launch"));
}

#[test]
fn burst_requests_coalesce_into_one_successful_cycle() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "burst");
    for _ in 0..3 {
        queue(&root, &db, 250, &[]);
    }
    let reader = wait_reader(&root, &db);
    assert!(
        reader.status.success(),
        "reader failed: {}",
        String::from_utf8_lossy(&reader.stderr)
    );
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 3);
    assert_eq!(state.completed_generation, 3);
    assert_eq!(state.successful_cycles, 1);
    assert_eq!(state.worker_launches, 1, "burst spawned a worker herd");
}

#[test]
fn request_during_update_runs_a_trailing_second_cycle() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "during");
    let started = temp.path().join("started");
    let delay = Path::new("350");
    queue(
        &root,
        &db,
        0,
        &[
            ("AST_INDEX_TEST_UPDATE_STARTED_FILE", &started),
            ("AST_INDEX_TEST_UPDATE_DELAY_MS", delay),
        ],
    );
    wait_for_file(&started);
    fs::write(root.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    queue(&root, &db, 0, &[]);
    let reader = wait_reader(&root, &db);
    assert!(
        reader.status.success(),
        "reader failed: {}",
        String::from_utf8_lossy(&reader.stderr)
    );
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 2);
    assert_eq!(state.completed_generation, 2);
    assert_eq!(state.successful_cycles, 2);
}

#[test]
fn failed_generation_stays_pending_and_a_later_request_retries_it() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "retry");
    let fail_once = temp.path().join("fail-once");
    fs::write(&fail_once, b"fail").unwrap();
    queue(
        &root,
        &db,
        0,
        &[("AST_INDEX_TEST_UPDATE_FAIL_ONCE_FILE", &fail_once)],
    );
    let failed_reader = run(
        &root,
        &db,
        &["stats"],
        &[("AST_INDEX_UPDATE_WAIT_TIMEOUT_MS", Path::new("3000"))],
    );
    assert!(!failed_reader.status.success());
    assert!(String::from_utf8_lossy(&failed_reader.stderr).contains("injected update failure"));
    let failed = read_state(&db);
    assert_eq!(failed.requested_generation, 1);
    assert_eq!(failed.completed_generation, 0);
    assert!(failed
        .last_error
        .as_deref()
        .unwrap()
        .contains("injected update failure"));

    queue(&root, &db, 0, &[]);
    let reader = wait_reader(&root, &db);
    assert!(
        reader.status.success(),
        "reader after retry failed: {}",
        String::from_utf8_lossy(&reader.stderr)
    );
    let retried = read_state(&db);
    assert_eq!(retried.requested_generation, 2);
    assert_eq!(retried.completed_generation, 2);
    assert_eq!(retried.successful_cycles, 1);
    assert!(retried.last_error.is_none());
}

#[test]
fn projects_have_independent_workers_and_generations() {
    let temp = TempDir::new().unwrap();
    let (root_a, db_a) = project(&temp, "project-a");
    let (root_b, db_b) = project(&temp, "project-b");
    let started = temp.path().join("a-started");
    let delay = Path::new("1500");
    queue(
        &root_a,
        &db_a,
        0,
        &[
            ("AST_INDEX_TEST_UPDATE_STARTED_FILE", &started),
            ("AST_INDEX_TEST_UPDATE_DELAY_MS", delay),
        ],
    );
    wait_for_file(&started);
    queue(&root_b, &db_b, 0, &[]);
    assert!(wait_reader(&root_b, &db_b).status.success());
    let state_a = read_state(&db_a);
    let state_b = read_state(&db_b);
    assert_eq!(state_a.requested_generation, 1);
    assert_eq!(state_a.completed_generation, 0);
    assert_eq!(state_b.completed_generation, 1);
    assert!(wait_reader(&root_a, &db_a).status.success());
}

#[test]
fn first_reader_waits_for_the_pending_generation() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "reader-wait");
    let started = temp.path().join("reader-started");
    let delay = Path::new("300");
    queue(
        &root,
        &db,
        0,
        &[
            ("AST_INDEX_TEST_UPDATE_STARTED_FILE", &started),
            ("AST_INDEX_TEST_UPDATE_DELAY_MS", delay),
        ],
    );
    wait_for_file(&started);
    let began = Instant::now();
    let reader = wait_reader(&root, &db);
    assert!(
        reader.status.success(),
        "reader failed: {}",
        String::from_utf8_lossy(&reader.stderr)
    );
    assert!(began.elapsed() >= Duration::from_millis(200));
    let state = read_state(&db);
    assert_eq!(state.completed_generation, state.requested_generation);
}

#[test]
fn worker_handoff_cannot_lose_request_at_exit_barrier() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "handoff");
    let before_exit = temp.path().join("before-exit");
    queue(
        &root,
        &db,
        0,
        &[
            ("AST_INDEX_TEST_UPDATE_BEFORE_EXIT_FILE", &before_exit),
            ("AST_INDEX_TEST_UPDATE_EXIT_DELAY_MS", Path::new("500")),
        ],
    );
    wait_for_file(&before_exit);
    fs::write(root.join("src/lib.rs"), "pub fn after_barrier() {}\n").unwrap();
    queue(&root, &db, 0, &[]);
    assert!(wait_reader(&root, &db).status.success());
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 2);
    assert_eq!(state.completed_generation, 2);
    assert_eq!(state.successful_cycles, 2);
}

#[test]
fn rebuild_acknowledges_only_its_start_generation_and_launches_newer_pending_work() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "rebuild-handoff");
    fs::write(
        state_path(&db),
        br#"{"version":1,"requested_generation":1,"completed_generation":0,"successful_cycles":0}"#,
    )
    .unwrap();
    let started = temp.path().join("refresh-started");
    let mut rebuild = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(&root)
        .env("AST_INDEX_DB_PATH", &db)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env("AST_INDEX_TEST_REFRESH_STARTED_FILE", &started)
        .env("AST_INDEX_TEST_REFRESH_DELAY_MS", "700")
        .arg("rebuild")
        .spawn()
        .unwrap();
    wait_for_file(&started);
    fs::write(root.join("src/lib.rs"), "pub fn newer_request() {}\n").unwrap();
    queue(&root, &db, 0, &[]);
    assert!(rebuild.wait().unwrap().success());
    let reader = wait_reader(&root, &db);
    assert!(
        reader.status.success(),
        "reader after rebuild handoff failed: {}",
        String::from_utf8_lossy(&reader.stderr)
    );
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 2);
    assert_eq!(state.completed_generation, 2);
    assert_eq!(
        state.successful_cycles, 1,
        "newer request must be updated, not incorrectly acknowledged by rebuild"
    );
}

#[test]
fn worker_log_hardlink_is_never_opened_or_truncated() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "safe-log");
    let target = temp.path().join("hardlink-target");
    fs::write(&target, b"must-survive").unwrap();
    let hostile_log = db
        .parent()
        .unwrap()
        .join(".ast-index-update-worker-attacker.log");
    fs::hard_link(&target, &hostile_log).unwrap();
    queue(&root, &db, 0, &[]);
    assert!(wait_reader(&root, &db).status.success());
    assert_eq!(fs::read(&target).unwrap(), b"must-survive");
    if hostile_log.exists() {
        assert_eq!(fs::read(&hostile_log).unwrap(), b"must-survive");
    }
}

#[test]
fn worker_killed_before_lock_is_recovered_by_the_next_request() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "prelock-crash");
    let before_lock = temp.path().join("before-lock");
    queue(
        &root,
        &db,
        0,
        &[
            ("AST_INDEX_TEST_WORKER_BEFORE_LOCK_FILE", &before_lock),
            ("AST_INDEX_TEST_WORKER_EXIT_BEFORE_LOCK", Path::new("1")),
        ],
    );
    wait_for_file(&before_lock);
    expire_worker_claim(&db);
    queue(&root, &db, 0, &[]);
    assert!(wait_reader(&root, &db).status.success());
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 2);
    assert_eq!(state.completed_generation, 2);
    assert_eq!(state.successful_cycles, 1);
    assert_eq!(state.worker_launches, 2);
}

#[test]
fn delayed_lock_handoff_uses_one_claimed_waiter() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "delayed-handoff");
    let before_exit = temp.path().join("delayed-before-exit");
    queue(
        &root,
        &db,
        0,
        &[
            ("AST_INDEX_TEST_UPDATE_BEFORE_EXIT_FILE", &before_exit),
            ("AST_INDEX_TEST_UPDATE_EXIT_DELAY_MS", Path::new("1200")),
        ],
    );
    wait_for_file(&before_exit);
    for _ in 0..8 {
        queue(&root, &db, 0, &[]);
    }
    assert!(wait_reader(&root, &db).status.success());
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 9);
    assert_eq!(state.completed_generation, 9);
    assert_eq!(state.worker_launches, 2, "handoff spawned a process herd");
}

#[test]
fn failed_old_launcher_cannot_clear_a_newer_recovered_claim() {
    let temp = TempDir::new().unwrap();
    let (root, db) = project(&temp, "launch-failure-race");
    let launch_started = temp.path().join("launch-started");
    let mut first = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(&root)
        .env("AST_INDEX_DB_PATH", &db)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("AST_INDEX_TEST_LAUNCH_STARTED_FILE", &launch_started)
        .env("AST_INDEX_TEST_LAUNCH_FAIL_DELAY_MS", "1500")
        .args(["update", "--background", "--debounce-ms", "0"])
        .spawn()
        .unwrap();
    wait_for_file(&launch_started);
    expire_worker_claim(&db);
    queue(&root, &db, 0, &[]);
    assert!(!first.wait().unwrap().success());
    assert!(wait_reader(&root, &db).status.success());
    let state = read_state(&db);
    assert_eq!(state.requested_generation, 2);
    assert_eq!(state.completed_generation, 2);
    assert_eq!(state.worker_launches, 2);
    assert!(state.last_error.is_none());
}
