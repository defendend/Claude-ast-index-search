//! `rebuild --type modules|deps|files` refreshes only its own tables; every
//! other table must survive into the published generation.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run(root: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(root)
        .args(args)
        .env("AST_INDEX_CACHE_DIR", root.parent().unwrap().join("cache"))
        .env("AST_INDEX_DISABLE_GC", "1")
        .env_remove("AST_INDEX_DB_PATH")
        .env_remove("KOTLIN_INDEX_DB_PATH")
        .env_remove("AST_INDEX_MAX_FILES")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn partial_rebuilds_keep_symbols_and_modules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("project");
    write(&root, "settings.gradle.kts", "include(\":app\", \":core\")\n");
    write(&root, "core/build.gradle.kts", "");
    write(
        &root,
        "app/build.gradle.kts",
        "dependencies { implementation(project(\":core\")) }\n",
    );
    write(&root, "core/src/main/kotlin/Repo.kt", "interface Repo\n");
    write(&root, "app/src/main/kotlin/RepoImpl.kt", "class RepoImpl : Repo\n");
    run(&root, &["rebuild"]);

    for index_type in ["modules", "deps", "files"] {
        run(&root, &["rebuild", "--type", index_type]);
        let class = run(&root, &["class", "RepoImpl"]);
        assert!(
            class.contains("app/src/main/kotlin/RepoImpl.kt"),
            "symbols lost after --type {index_type}: {class}"
        );
        let deps = run(&root, &["deps", "app"]);
        assert!(
            deps.contains("core"),
            "module deps lost after --type {index_type}: {deps}"
        );
    }
}
