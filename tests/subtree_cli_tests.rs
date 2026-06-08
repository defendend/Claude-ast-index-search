//! Step 2 of #31: CLI surface for named subtrees (`subtree add/remove/list`).
//! Drives the binary end-to-end so we cover clap routing and stdout/stderr
//! formatting on top of the DB layer covered in subtree_schema_tests.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_ast-index"))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn rebuild(root: &Path) {
    let out = Command::new(binary())
        .current_dir(root)
        .args(["rebuild"])
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "rebuild failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(root)
        .args(args)
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap()
}

#[test]
fn add_list_remove_round_trip() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let extra = tmp.path().join("extra");
    write(&project.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    write(&project.join("src/lib.rs"), "pub fn a() {}\n");
    write(&extra.join("Cargo.toml"), "[package]\nname=\"y\"\nversion=\"0\"\n");
    write(&extra.join("src/lib.rs"), "pub fn b() {}\n");

    rebuild(&project);

    // Initially no subtrees.
    let list = run(&project, &["subtree", "list"]);
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("(primary)"));
    assert!(stdout.contains("No extra subtrees attached"));

    // Add the sibling extra/.
    let add = run(&project, &["subtree", "add", "extra", "../extra"]);
    assert!(
        add.status.success(),
        "subtree add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout);
    assert!(stdout.contains("Attached subtree extra"));
    assert!(stdout.contains("source: ../extra"), "original path should appear");

    // List shows it now.
    let list = run(&project, &["subtree", "list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("extra"));
    assert!(stdout.contains("../extra"));

    // JSON list returns proper structure.
    let json = run(&project, &["--format", "json", "subtree", "list"]);
    let stdout = String::from_utf8_lossy(&json.stdout);
    assert!(stdout.contains("\"name\": \"extra\""));
    assert!(stdout.contains("\"original_path\": \"../extra\""));

    // Remove and confirm.
    let remove = run(&project, &["subtree", "remove", "extra"]);
    assert!(remove.status.success());
    let stdout = String::from_utf8_lossy(&remove.stdout);
    assert!(stdout.contains("Detached subtree extra"));

    let list = run(&project, &["subtree", "list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("No extra subtrees attached"));
}

#[test]
fn add_with_duplicate_name_rejects() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let extra1 = tmp.path().join("extra1");
    let extra2 = tmp.path().join("extra2");
    write(&project.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    write(&project.join("src/lib.rs"), "pub fn a() {}\n");
    write(&extra1.join("a.rs"), "fn x() {}\n");
    write(&extra2.join("b.rs"), "fn y() {}\n");

    rebuild(&project);

    let first = run(&project, &["subtree", "add", "core", "../extra1"]);
    assert!(first.status.success());

    let dup = run(&project, &["subtree", "add", "core", "../extra2"]);
    assert!(dup.status.success(), "should not crash on duplicate name");
    let stdout = String::from_utf8_lossy(&dup.stdout);
    assert!(
        stdout.contains("already attached"),
        "expected duplicate-name warning, got: {stdout}"
    );

    // Only one subtree should be in the index.
    let list = run(&project, &["--format", "json", "subtree", "list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 1);
}

#[test]
fn add_overlapping_with_root_rejects_without_force() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    write(&project.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    write(&project.join("src/lib.rs"), "pub fn a() {}\n");
    write(&project.join("sub/a.rs"), "fn x() {}\n");

    rebuild(&project);

    // Without --force we refuse to attach a nested directory.
    let inside = run(&project, &["subtree", "add", "inner", "./sub"]);
    assert!(inside.status.success());
    let stdout = String::from_utf8_lossy(&inside.stdout);
    assert!(
        stdout.contains("inside the project root"),
        "expected overlap warning, got: {stdout}"
    );

    // With --force the attach goes through.
    let forced = run(
        &project,
        &["subtree", "add", "inner", "./sub", "--force"],
    );
    assert!(forced.status.success());
    let stdout = String::from_utf8_lossy(&forced.stdout);
    assert!(stdout.contains("Attached subtree inner"));
}

#[test]
fn legacy_add_root_still_works_and_auto_names() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let extra = tmp.path().join("legacy");
    write(&project.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0\"\n");
    write(&project.join("src/lib.rs"), "pub fn a() {}\n");
    write(&extra.join("a.rs"), "fn x() {}\n");

    rebuild(&project);

    let add = run(&project, &["add-root", "../legacy"]);
    assert!(add.status.success());

    let list = run(&project, &["subtree", "list"]);
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains("legacy"),
        "auto-name from path basename, got: {stdout}"
    );
}
