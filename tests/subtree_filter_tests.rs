//! Step 4 of #31: `--subtree NAME` and `--local` narrow search results to
//! a single subtree (or to the primary project only).

use std::fs;
use std::path::Path;
use std::process::Command;

use ast_index::db as index_db;
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

fn run(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .current_dir(cwd)
        .args(args)
        .env(
            "AST_INDEX_CACHE_DIR",
            cwd.parent().unwrap_or(cwd).join("ast-index-test-cache"),
        )
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap()
}

fn make_workspace() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("main");
    let extra = tmp.path().join("extra");
    write(
        &main.join("Cargo.toml"),
        "[package]\nname=\"m\"\nversion=\"0\"\n",
    );
    write(&main.join("src/lib.rs"), "pub fn shared_fn() {}\n");
    write(
        &extra.join("Cargo.toml"),
        "[package]\nname=\"e\"\nversion=\"0\"\n",
    );
    write(&extra.join("src/lib.rs"), "pub fn shared_fn() {}\n");

    assert!(run(&main, &["rebuild"]).status.success());
    assert!(run(&main, &["subtree", "add", "extra", "../extra"])
        .status
        .success());
    assert!(run(&main, &["rebuild"]).status.success());

    let main_path = main.clone();
    (tmp, main_path)
}

fn parse_json(stdout: &[u8]) -> serde_json::Value {
    serde_json::from_slice(stdout).expect("expected valid JSON output")
}

fn make_crowded_workspace() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("main");
    let extra = tmp.path().join("extra");
    write(
        &main.join("Cargo.toml"),
        "[package]\nname=\"m\"\nversion=\"0\"\n",
    );
    let mut primary = String::from("pub trait CrowdBase {}\n");
    for index in 0..6 {
        primary.push_str(&format!(
            "pub mod m{index} {{ pub struct CrowdType; pub struct CrowdImpl; impl super::CrowdBase for CrowdImpl {{}} pub fn crowd_target() {{}} pub fn caller() {{ crowd_target(); }} }}\n"
        ));
    }
    write(&main.join("src/lib.rs"), &primary);
    write(
        &main.join("src/Usage.kt"),
        "fun use0() { crowd_target() }\nfun use1() { crowd_target() }\nfun use2() { crowd_target() }\nfun use3() { crowd_target() }\nfun use4() { crowd_target() }\nfun use5() { crowd_target() }\n",
    );
    write(
        &extra.join("Cargo.toml"),
        "[package]\nname=\"e\"\nversion=\"0\"\n",
    );
    write(
        &extra.join("src/lib.rs"),
        "pub trait CrowdBase {}\npub struct CrowdType;\npub struct CrowdImpl;\nimpl CrowdBase for CrowdImpl {}\npub fn crowd_target() {}\npub fn caller() { crowd_target(); }\n",
    );
    write(
        &extra.join("src/Usage.kt"),
        "fun subtreeUse() { crowd_target() }\n",
    );
    assert!(run(&main, &["rebuild"]).status.success());
    assert!(run(&main, &["subtree", "add", "extra", "../extra"])
        .status
        .success());
    assert!(run(&main, &["rebuild"]).status.success());
    let db_path = String::from_utf8(run(&main, &["db-path"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute("DELETE FROM refs WHERE name = 'crowd_target'", [])
        .unwrap();
    let primary_root = index_db::normalize_root_for_storage(&main);
    let extra_root = index_db::normalize_root_for_storage(&extra);
    for index in 0..6 {
        conn.execute(
            "INSERT INTO files(path, root_path, mtime, size) VALUES (?1, ?2, 1, 1)",
            rusqlite::params![format!("seed/main-{index}.kt"), primary_root],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO refs(name, file_id, line, context) VALUES ('crowd_target', ?1, 1, ?2)",
            rusqlite::params![file_id, format!("mainUse{index}")],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO files(path, root_path, mtime, size) VALUES ('seed/extra.kt', ?1, 1, 1)",
        [&extra_root],
    )
    .unwrap();
    let file_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO refs(name, file_id, line, context) VALUES ('crowd_target', ?1, 1, 'subtreeUse')",
        [file_id],
    )
    .unwrap();
    for index in 0..6 {
        conn.execute(
            "INSERT INTO files(path, root_path, mtime, size) VALUES (?1, ?2, 1, 1)",
            rusqlite::params![format!("seed/foreign-symbol-{index}.rs"), extra_root],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO symbols(file_id, name, kind, line, signature) VALUES (?1, 'local_crowd', 'function', 1, 'FOREIGN')",
            [file_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO refs(name, file_id, line, context) VALUES ('local_crowd', ?1, 1, 'FOREIGN_USE')",
            [file_id],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO files(path, root_path, mtime, size) VALUES ('seed/primary-symbol.rs', ?1, 1, 1)",
        [&primary_root],
    )
    .unwrap();
    let file_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO symbols(file_id, name, kind, line, signature) VALUES (?1, 'local_crowd', 'function', 1, 'PRIMARY_VALID')",
        [file_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO refs(name, file_id, line, context) VALUES ('local_crowd', ?1, 1, 'PRIMARY_USE')",
        [file_id],
    )
    .unwrap();
    drop(conn);
    let main_path = main.clone();
    (tmp, main_path)
}

#[test]
fn no_filter_returns_both() {
    let (_tmp, main) = make_workspace();
    let out = run(&main, &["--format", "json", "symbol", "shared_fn"]);
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    let names: Vec<&str> = parsed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "both copies should be found: {names:?}");
}

#[test]
fn local_flag_keeps_only_primary() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &["--local", "--format", "json", "symbol", "shared_fn"],
    );
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    let arr = parsed["items"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "--local must drop the subtree row");
    let path = arr[0]["path"].as_str().unwrap();
    assert!(path.contains("/main/"), "expected primary path, got {path}");
}

#[test]
fn subtree_flag_keeps_only_named_subtree() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &[
            "--subtree",
            "extra",
            "--format",
            "json",
            "symbol",
            "shared_fn",
        ],
    );
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    let arr = parsed["items"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "--subtree extra must keep only extra rows");
    let path = arr[0]["path"].as_str().unwrap();
    assert!(
        path.contains("/extra/"),
        "expected subtree path, got {path}"
    );
}

#[test]
fn subtree_flag_with_unknown_name_returns_empty() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &[
            "--subtree",
            "ghost",
            "--format",
            "json",
            "symbol",
            "shared_fn",
        ],
    );
    assert!(out.status.success());
    let parsed = parse_json(&out.stdout);
    assert!(parsed["items"].as_array().unwrap().is_empty());
}

#[test]
fn local_and_subtree_conflict() {
    let (_tmp, main) = make_workspace();
    let out = run(
        &main,
        &["--local", "--subtree", "extra", "symbol", "shared_fn"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mutually exclusive"),
        "expected conflict error, got: {stderr}"
    );
}

#[test]
fn subtree_filter_is_applied_before_limit_across_index_commands() {
    let (_tmp, main) = make_crowded_workspace();
    let commands: &[&[&str]] = &[
        &["symbol", "crowd_target", "--limit", "1"],
        &["class", "CrowdType", "--limit", "1"],
        &["implementations", "CrowdBase", "--limit", "1"],
        &["usages", "crowd_target", "--limit", "1"],
        &["refs", "crowd_target", "--limit", "1"],
        &["search", "crowd_target", "--limit", "1"],
    ];
    for command in commands {
        let mut args = vec!["--subtree", "extra", "--format", "json"];
        args.extend_from_slice(command);
        let output = run(&main, &args);
        assert!(
            output.status.success(),
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value = parse_json(&output.stdout);
        if command[0] == "usages" {
            let items = value["items"].as_array().unwrap();
            assert_eq!(items.len(), 1, "scoped usage page was not filled: {value}");
            assert!(
                items[0]["context"]
                    .as_str()
                    .unwrap_or("")
                    .contains("subtreeUse"),
                "unexpected scoped usage: {value}"
            );
            continue;
        }
        if command[0] == "refs" {
            let usages = value["usages"].as_array().unwrap();
            assert_eq!(usages.len(), 1, "scoped refs page was not filled: {value}");
            assert!(usages[0]["context"]
                .as_str()
                .unwrap_or("")
                .contains("subtreeUse"));
            let definitions = value["definitions"].as_array().unwrap();
            assert_eq!(definitions.len(), 1);
            assert!(definitions[0]["path"].as_str().unwrap().contains("/extra/"));
            continue;
        }
        let paths: Vec<&str> = match command[0] {
            "refs" => ["definitions", "imports", "usages"]
                .iter()
                .flat_map(|key| value[*key].as_array().unwrap().iter())
                .filter_map(|item| item["path"].as_str())
                .collect(),
            "search" => value["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item["path"].as_str())
                .collect(),
            _ => value["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item["path"].as_str())
                .collect(),
        };
        assert!(
            !paths.is_empty(),
            "{command:?} lost the valid subtree row: {value}"
        );
        assert!(
            paths.iter().all(|path| path.contains("/extra/")),
            "{command:?} leaked primary rows before LIMIT: {paths:?}"
        );
    }

    let local = run(
        &main,
        &[
            "--local",
            "--format",
            "json",
            "symbol",
            "local_crowd",
            "--limit",
            "1",
        ],
    );
    assert!(local.status.success());
    let local = parse_json(&local.stdout);
    assert_eq!(local["pagination"]["total"], 1);
    assert_eq!(local["items"].as_array().unwrap().len(), 1);
    assert_eq!(local["items"][0]["signature"], "PRIMARY_VALID");

    let local_usages = run(
        &main,
        &[
            "--local",
            "--format",
            "json",
            "usages",
            "local_crowd",
            "--limit",
            "1",
        ],
    );
    assert!(local_usages.status.success());
    let local_usages = parse_json(&local_usages.stdout);
    assert_eq!(local_usages["pagination"]["total"], 1);
    assert_eq!(local_usages["items"].as_array().unwrap().len(), 1);
    assert_eq!(local_usages["items"][0]["context"], "PRIMARY_USE");

    let local_refs = run(
        &main,
        &[
            "--local",
            "--format",
            "json",
            "refs",
            "local_crowd",
            "--limit",
            "1",
        ],
    );
    assert!(local_refs.status.success());
    let local_refs = parse_json(&local_refs.stdout);
    assert_eq!(local_refs["pagination"]["definitions"]["total"], 1);
    assert_eq!(local_refs["pagination"]["usages"]["total"], 1);
    assert_eq!(local_refs["definitions"][0]["signature"], "PRIMARY_VALID");
    assert_eq!(local_refs["usages"][0]["context"], "PRIMARY_USE");
}
