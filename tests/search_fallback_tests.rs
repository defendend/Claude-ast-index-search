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
    let admin = project
        .path()
        .join("trust/faust/internal/actions/administration");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(project.path().join("market/other")).unwrap();
    fs::write(project.path().join("go.mod"), "module fixture\n").unwrap();
    fs::write(
        admin.join("update_administration_processing.go"),
        "package administration\n\nfunc UpdateAdministrationProcessing(ctx Context, request Req) (*Resp, error) {\n\treturn nil, nil\n}\n",
    )
    .unwrap();
    fs::write(
        admin.join("get_administration_processing.go"),
        "package administration\n\nfunc GetAdministrationProcessing(ctx Context, request Req) (*Resp, error) {\n\treturn nil, nil\n}\n",
    )
    .unwrap();
    fs::write(
        project.path().join("market/other/processing.go"),
        "package other\n\nfunc MarketProcessingUpdate() {}\n",
    )
    .unwrap();
    let rebuild = run(project.path(), cache.path(), &["rebuild"]);
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    (project, cache)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

const INTENT_QUERY: &str = "processing cc administration processing update get";

#[test]
fn multi_word_query_falls_back_to_ranked_results_instead_of_nothing() {
    let (project, cache) = fixture();
    let out = stdout(&run(
        project.path(),
        cache.path(),
        &["search", INTENT_QUERY],
    ));
    assert!(!out.contains("No results found"), "{out}");
    assert!(out.contains("No literal matches"), "{out}");
    assert!(out.contains("UpdateAdministrationProcessing"), "{out}");
    assert!(out.contains("GetAdministrationProcessing"), "{out}");
}

#[test]
fn fallback_honours_module_scope() {
    let (project, cache) = fixture();
    let scoped = stdout(&run(
        project.path(),
        cache.path(),
        &["search", "processing update", "--module", "trust/faust"],
    ));
    assert!(
        scoped.contains("UpdateAdministrationProcessing"),
        "{scoped}"
    );
    assert!(!scoped.contains("MarketProcessingUpdate"), "{scoped}");

    let unscoped = stdout(&run(
        project.path(),
        cache.path(),
        &["search", "processing update"],
    ));
    assert!(unscoped.contains("MarketProcessingUpdate"), "{unscoped}");
}

#[test]
fn single_term_query_keeps_literal_semantics() {
    let (project, cache) = fixture();
    let missing = stdout(&run(
        project.path(),
        cache.path(),
        &["search", "ZzzNothingHere"],
    ));
    assert!(missing.contains("No results found"), "{missing}");
    assert!(!missing.contains("No literal matches"), "{missing}");

    let exact = stdout(&run(
        project.path(),
        cache.path(),
        &["search", "UpdateAdministrationProcessing"],
    ));
    assert!(exact.starts_with("Search results for"), "{exact}");
    assert!(!exact.contains("No literal matches"), "{exact}");
}

#[test]
fn fallback_json_is_one_document_with_explicit_marker() {
    let (project, cache) = fixture();
    let out = stdout(&run(
        project.path(),
        cache.path(),
        &["--format", "json", "search", INTENT_QUERY],
    ));
    let doc: Value = serde_json::from_str(&out).expect("single valid JSON document");
    assert_eq!(doc["fallback"], "explore");
    assert!(doc["reason"].as_str().unwrap().contains("multi-word"));
    let names: Vec<&str> = doc["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(
        names.contains(&"UpdateAdministrationProcessing"),
        "{names:?}"
    );
}

#[test]
fn plain_explore_json_has_no_fallback_marker() {
    let (project, cache) = fixture();
    let out = stdout(&run(
        project.path(),
        cache.path(),
        &["--format", "json", "explore", INTENT_QUERY],
    ));
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert!(doc.get("fallback").is_none());
}
