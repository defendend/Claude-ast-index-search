use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
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

fn run(cwd: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(cwd)
        .args(args)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap()
}

fn content_paths(output: &Output) -> Vec<String> {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    json["content_matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap().to_string())
        .collect()
}

struct Workspace {
    _tmp: TempDir,
    project: PathBuf,
    caller: PathBuf,
    cache: PathBuf,
}

fn workspace() -> Workspace {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let extra = tmp.path().join("extra");
    let caller = tmp.path().join("caller");
    let cache = tmp.path().join("cache");

    write(
        &project.join("Cargo.toml"),
        "[package]\nname = \"search-scope\"\nversion = \"0.1.0\"\n",
    );
    write(
        &project.join(".ast-index.yaml"),
        "include:\n  - included\nroots:\n  - ../extra\n",
    );
    write(
        &project.join("included/main.rs"),
        "fn primary() { let _ = \"scope_sentinel\"; }\n",
    );
    write(
        &project.join("excluded/ignored.rs"),
        "fn excluded() { let _ = \"scope_sentinel\"; }\n",
    );
    write(
        &extra.join("lib.rs"),
        "fn extra() { let _ = \"scope_sentinel\"; }\n",
    );
    fs::create_dir_all(&caller).unwrap();

    let project_arg = project.to_str().unwrap();
    let rebuild = run(&caller, &cache, &["--root", project_arg, "rebuild"]);
    assert!(
        rebuild.status.success(),
        "rebuild failed: {}",
        String::from_utf8_lossy(&rebuild.stderr)
    );

    Workspace {
        _tmp: tmp,
        project,
        caller,
        cache,
    }
}

#[test]
fn search_honors_include_and_searches_configured_roots() {
    let workspace = workspace();
    let project = workspace.project.to_str().unwrap();
    let output = run(
        &workspace.caller,
        &workspace.cache,
        &[
            "--root",
            project,
            "--format",
            "json",
            "search",
            "scope_sentinel",
        ],
    );
    let paths = content_paths(&output);

    assert_eq!(paths.len(), 2, "unexpected content matches: {paths:?}");
    assert!(paths.iter().any(|path| path.contains("included/main.rs")));
    assert!(paths.iter().any(|path| path.contains("extra/lib.rs")));
    assert!(!paths.iter().any(|path| path.contains("excluded")));
}

#[test]
fn local_and_subtree_flags_limit_content_search_roots() {
    let workspace = workspace();
    let project = workspace.project.to_str().unwrap();

    let local = run(
        &workspace.caller,
        &workspace.cache,
        &[
            "-C",
            project,
            "--local",
            "--format",
            "json",
            "search",
            "scope_sentinel",
        ],
    );
    let local_paths = content_paths(&local);
    assert_eq!(
        local_paths.len(),
        1,
        "unexpected local matches: {local_paths:?}"
    );
    assert!(local_paths[0].contains("included/main.rs"));

    let subtree = run(
        &workspace.caller,
        &workspace.cache,
        &[
            "--root",
            project,
            "--subtree",
            "extra",
            "--format",
            "json",
            "search",
            "scope_sentinel",
        ],
    );
    let subtree_paths = content_paths(&subtree);
    assert_eq!(
        subtree_paths.len(),
        1,
        "unexpected subtree matches: {subtree_paths:?}"
    );
    assert!(subtree_paths[0].contains("extra/lib.rs"));
}

#[test]
fn search_rejects_invalid_scope_before_walking() {
    let workspace = workspace();
    write(
        &workspace.project.join(".ast-index.yaml"),
        "include:\n  - missing\n",
    );
    let project = workspace.project.to_str().unwrap();
    let output = run(
        &workspace.caller,
        &workspace.cache,
        &["--root", project, "search", "scope_sentinel"],
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("include path does not exist"), "{stderr}");
}

#[test]
fn explicit_root_rejects_missing_directory() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("missing");
    let cache = tmp.path().join("cache");
    let output = run(
        tmp.path(),
        &cache,
        &["--root", missing.to_str().unwrap(), "stats"],
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("project root does not exist"), "{stderr}");
}
