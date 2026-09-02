use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .args(args)
        .output()
        .unwrap()
}

fn fixture() -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    fs::create_dir(project.path().join("src")).unwrap();
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname=\"pagination-fixture\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        project.path().join("src/lib.rs"),
        r#"
pub trait PaginationBase {}

pub mod one {
    pub struct PaginationClass;
    pub struct PaginationImpl;
    impl super::PaginationBase for PaginationImpl {}
    pub fn pagination_target() {}
    pub fn caller() { pagination_target(); }
}

pub mod two {
    pub struct PaginationClass;
    pub struct PaginationImpl;
    impl super::PaginationBase for PaginationImpl {}
    pub fn pagination_target() {}
    pub fn caller() { pagination_target(); }
}

pub mod three {
    pub struct PaginationClass;
    pub struct PaginationImpl;
    impl super::PaginationBase for PaginationImpl {}
    pub fn pagination_target() {}
    pub fn caller() { pagination_target(); }
}
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("src/Usage.kt"),
        r#"
fun kotlinCallerOne() { pagination_target() }
fun kotlinCallerTwo() { pagination_target() }
fun kotlinCallerThree() { pagination_target() }
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("src/fuzzy_outside.rs"),
        r#"
pub fn ScopedNeedleA() {}
pub fn ScopedNeedleB() {}
pub fn ScopedNeedleC() {}
pub struct ScopedNeedleClassA;
pub struct ScopedNeedleClassB;
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("src/fuzzy_scoped.rs"),
        "pub fn ScopedNeedleTarget() {}\npub struct ScopedNeedleClassTarget;\n",
    )
    .unwrap();
    let rebuild = run(project.path(), cache.path(), &["rebuild"]);
    assert!(
        rebuild.status.success(),
        "rebuild failed: {}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    (project, cache)
}

fn assert_text_truncated(root: &Path, cache: &Path, args: &[&str]) {
    let output = run(root, cache, args);
    assert!(
        output.status.success(),
        "command {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("showing 1 of"),
        "command {args:?} did not report shown vs total:\n{stdout}"
    );
    assert!(
        stdout.contains("Truncated:") && stdout.contains("--limit"),
        "command {args:?} omitted truncation guidance:\n{stdout}"
    );
}

fn assert_json_page(value: &Value, command: &[&str]) {
    assert_eq!(value["schema_version"], 2, "command {command:?}: {value}");
    assert_eq!(
        value["pagination"]["returned"], 1,
        "command {command:?}: {value}"
    );
    assert_eq!(
        value["pagination"]["truncated"], true,
        "command {command:?}: {value}"
    );
    assert!(
        value["pagination"]["total"].as_u64().unwrap() > 1,
        "command {command:?}: {value}"
    );
    assert_eq!(
        value["items"].as_array().unwrap().len(),
        1,
        "command {command:?}: {value}"
    );
}

#[test]
fn seven_search_commands_surface_text_and_json_truncation() {
    let (project, cache) = fixture();
    let commands: &[&[&str]] = &[
        &["search", "pagination_target", "--limit", "1"],
        &["symbol", "pagination_target", "--limit", "1"],
        &["class", "PaginationClass", "--limit", "1"],
        &["implementations", "PaginationBase", "--limit", "1"],
        &["refs", "pagination_target", "--limit", "1"],
        &["usages", "pagination_target", "--limit", "1"],
        &["callers", "pagination_target", "--limit", "1"],
    ];

    for command in commands {
        assert_text_truncated(project.path(), cache.path(), command);

        let mut json_args = vec!["--format", "json"];
        json_args.extend_from_slice(command);
        let output = run(project.path(), cache.path(), &json_args);
        assert!(
            output.status.success(),
            "JSON command {command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "JSON command {command:?} returned invalid JSON: {error}; stdout={}",
                String::from_utf8_lossy(&output.stdout)
            )
        });

        match command[0] {
            "search" => {
                assert_eq!(value["schema_version"], 2);
                let pages = value["pagination"].as_object().unwrap();
                assert!(pages.values().any(|page| {
                    page["returned"] == 1
                        && page["truncated"] == true
                        && page["total"].as_u64().unwrap_or(0) > 1
                }));
            }
            "refs" => {
                assert_eq!(value["schema_version"], 2);
                let pages = value["pagination"].as_object().unwrap();
                assert!(pages.values().any(|page| {
                    page["returned"] == 1
                        && page["truncated"] == true
                        && page["total"].as_u64().unwrap_or(0) > 1
                }));
            }
            _ => assert_json_page(&value, command),
        }
    }

    let kind_filtered = run(
        project.path(),
        cache.path(),
        &[
            "--format",
            "json",
            "search",
            "Pagination",
            "--type",
            "class",
            "--limit",
            "1",
        ],
    );
    assert!(
        kind_filtered.status.success(),
        "kind-filtered search failed: {}",
        String::from_utf8_lossy(&kind_filtered.stderr)
    );
    let value: Value = serde_json::from_slice(&kind_filtered.stdout).unwrap();
    assert_eq!(value["pagination"]["symbols"]["returned"], 1);
    assert!(value["pagination"]["symbols"]["total"].as_u64().unwrap() > 1);
    assert_eq!(value["pagination"]["symbols"]["truncated"], true);
    assert_eq!(value["symbols"][0]["kind"], "class");

    for pattern_command in [
        ["symbol", "--pattern", "Pagination*", "--limit", "1"],
        ["class", "--pattern", "Pagination*", "--limit", "1"],
    ] {
        let mut args = vec!["--format", "json"];
        args.extend_from_slice(&pattern_command);
        let output = run(project.path(), cache.path(), &args);
        assert!(
            output.status.success(),
            "pattern command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_json_page(&value, &pattern_command);
    }

    for (scoped_fuzzy, expected) in [
        (
            [
                "symbol",
                "ScopedNeedle",
                "--fuzzy",
                "--in-file",
                "fuzzy_scoped.rs",
            ],
            2,
        ),
        (
            [
                "class",
                "ScopedNeedle",
                "--fuzzy",
                "--in-file",
                "fuzzy_scoped.rs",
            ],
            1,
        ),
    ] {
        let mut args = vec!["--format", "json"];
        args.extend_from_slice(&scoped_fuzzy);
        let output = run(project.path(), cache.path(), &args);
        assert!(
            output.status.success(),
            "scoped fuzzy command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["pagination"]["total"], expected);
        assert_eq!(value["items"].as_array().unwrap().len(), expected);
        assert!(value["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["path"].as_str().unwrap().ends_with("fuzzy_scoped.rs")));
    }

    let single = run(
        project.path(),
        cache.path(),
        &["--format", "json", "search", "Pagination", "--limit", "5"],
    );
    let overlap = run(
        project.path(),
        cache.path(),
        &[
            "--format",
            "json",
            "search",
            "PaginationClass,Pagination",
            "--limit",
            "5",
        ],
    );
    assert!(single.status.success() && overlap.status.success());
    let single: Value = serde_json::from_slice(&single.stdout).unwrap();
    let overlap: Value = serde_json::from_slice(&overlap.stdout).unwrap();
    assert_eq!(
        overlap["pagination"]["symbols"]["total"], single["pagination"]["symbols"]["total"],
        "overlapping OR terms must count each symbol once"
    );
    assert_eq!(overlap["symbols"].as_array().unwrap().len(), 5);
}
