//! `rebuild` records what the project is, and `stats` / `map` report it.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .env("AST_INDEX_CACHE_DIR", cache)
        .env("AST_INDEX_DISABLE_GC", "1")
        .env("NO_COLOR", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn stats_and_map_name_the_stacks_found_at_rebuild() {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = project.path();
    fs::write(root.join("Gemfile"), "source 'https://rubygems.org'\n").unwrap();
    fs::write(root.join("package.json"), "{}\n").unwrap();
    fs::create_dir_all(root.join("app/models")).unwrap();
    fs::write(root.join("app/models/invoice.rb"), "class Invoice\nend\n").unwrap();
    fs::create_dir_all(root.join("engines/billing")).unwrap();
    fs::write(root.join("engines/billing/billing.gemspec"), "").unwrap();
    run(root, cache.path(), &["rebuild"]);

    let stats: serde_json::Value =
        serde_json::from_str(&run(root, cache.path(), &["--format", "json", "stats"])).unwrap();
    let label = stats["project"].as_str().unwrap();
    assert!(label.contains("Ruby") && label.contains("Web"), "{label}");
    assert_eq!(stats["stats"]["module_count"], 1);

    let text = run(root, cache.path(), &["stats"]);
    assert!(text.contains(&format!("  Project:    {label}\n")), "{text}");

    let map = run(root, cache.path(), &["map"]);
    assert!(map.starts_with(&format!("Project: {label} | ")), "{map}");
}
