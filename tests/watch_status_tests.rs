use std::fs;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ast-index"))
}

fn run(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .args(args)
        .output()
        .unwrap()
}

struct WatchChild(Child);

impl Drop for WatchChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn watcher_for_project_a_does_not_suppress_project_b() {
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let project_a = temp.path().join("project-a");
    let project_b = temp.path().join("project-b");
    for (root, function) in [(&project_a, "a"), (&project_b, "b")] {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname=\"{function}\"\nversion=\"0.1.0\"\n"),
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            format!("pub fn {function}() {{}}\n"),
        )
        .unwrap();
        let rebuild = run(root, &cache, &["rebuild"]);
        assert!(
            rebuild.status.success(),
            "rebuild failed for {}: {}",
            root.display(),
            String::from_utf8_lossy(&rebuild.stderr)
        );
    }

    let mut watcher = WatchChild(
        Command::new(binary())
            .current_dir(&project_a)
            .env("AST_INDEX_CACHE_DIR", &cache)
            .env("AST_INDEX_DISABLE_GC", "1")
            .env_remove("AST_INDEX_DB_PATH")
            .env_remove("KOTLIN_INDEX_DB_PATH")
            .arg("watch")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = watcher.0.try_wait().unwrap() {
            panic!("watcher exited before acquiring its lock: {status}");
        }
        if run(&project_a, &cache, &["watch-status", "--quiet"])
            .status
            .success()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "watcher did not acquire its lock"
        );
        thread::sleep(Duration::from_millis(50));
    }

    let project_a_status = run(&project_a, &cache, &["watch-status"]);
    assert!(project_a_status.status.success());
    assert_eq!(project_a_status.stdout, b"watching\n");

    let project_b_status = run(&project_b, &cache, &["watch-status"]);
    assert_eq!(project_b_status.status.code(), Some(1));
    assert_eq!(project_b_status.stdout, b"not-watching\n");
    assert!(project_b_status.stderr.is_empty());

    let project_b_json = run(&project_b, &cache, &["--format", "json", "watch-status"]);
    assert_eq!(project_b_json.status.code(), Some(1));
    assert_eq!(project_b_json.stdout, b"{\"watching\":false}\n");
    assert!(project_b_json.stderr.is_empty());

    let project_b_quiet = run(&project_b, &cache, &["watch-status", "--quiet"]);
    assert_eq!(project_b_quiet.status.code(), Some(1));
    assert!(project_b_quiet.stdout.is_empty());
    assert!(project_b_quiet.stderr.is_empty());
}
