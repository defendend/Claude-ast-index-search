use std::fs;
use std::path::{Path, PathBuf};

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn write_module(root: &Path, module_path: &str, build_script: &str) -> PathBuf {
    let build_file = root.join(module_path).join("build.gradle.kts");
    fs::create_dir_all(build_file.parent().unwrap()).unwrap();
    fs::write(&build_file, build_script).unwrap();
    build_file
}

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    conn
}

#[test]
fn type_safe_accessors_resolve_gradle_word_boundaries_in_the_observable_graph() {
    let project = TempDir::new().unwrap();
    let root = project.path();
    let files = vec![
        write_module(root, "core/design-icon", ""),
        write_module(root, "feature/design_icon", ""),
        write_module(root, "tooling/design.tokens", ""),
        write_module(root, "core/network", ""),
        write_module(
            root,
            "app",
            r#"
                dependencies {
                    implementation(projects.core.designIcon)
                    api(projects.feature.designIcon)
                    compileOnly(projects.tooling.designTokens)
                    testImplementation(projects.core.network)
                }
            "#,
        ),
    ];

    let mut conn = fresh_db();
    indexer::index_modules_from_files(&conn, root, &files).unwrap();
    let indexed = indexer::index_module_dependencies(&mut conn, root, &files, false).unwrap();

    assert_eq!(indexed, 4);
    let deps = indexer::get_module_deps(&conn, "app").unwrap();
    assert_eq!(
        deps,
        vec![
            (
                "feature.design_icon".to_string(),
                "feature/design_icon".to_string(),
                "api".to_string(),
            ),
            (
                "tooling.design.tokens".to_string(),
                "tooling/design.tokens".to_string(),
                "compileOnly".to_string(),
            ),
            (
                "core.design-icon".to_string(),
                "core/design-icon".to_string(),
                "implementation".to_string(),
            ),
            (
                "core.network".to_string(),
                "core/network".to_string(),
                "testImplementation".to_string(),
            ),
        ]
    );

    for module in [
        "core.design-icon",
        "feature.design_icon",
        "tooling.design.tokens",
        "core.network",
    ] {
        assert_eq!(
            indexer::get_module_dependents(&conn, module).unwrap(),
            vec![(
                "app".to_string(),
                "app".to_string(),
                deps.iter()
                    .find(|(name, _, _)| name == module)
                    .unwrap()
                    .2
                    .clone(),
            )],
            "missing reverse edge for {module}"
        );
    }
}

#[test]
fn ambiguous_normalized_accessor_is_skipped_while_exact_lookup_is_preserved() {
    let project = TempDir::new().unwrap();
    let root = project.path();
    let files = vec![
        write_module(root, "core/design-icon", ""),
        write_module(root, "core/design_icon", ""),
        write_module(
            root,
            "app",
            r#"
                dependencies {
                    implementation(projects.core.designIcon)
                    api(projects.core.design_icon)
                }
            "#,
        ),
    ];

    let mut conn = fresh_db();
    indexer::index_modules_from_files(&conn, root, &files).unwrap();
    let indexed = indexer::index_module_dependencies(&mut conn, root, &files, false).unwrap();

    assert_eq!(indexed, 1);
    assert_eq!(
        indexer::get_module_deps(&conn, "app").unwrap(),
        vec![(
            "core.design_icon".to_string(),
            "core/design_icon".to_string(),
            "api".to_string(),
        )]
    );
    assert!(indexer::get_module_dependents(&conn, "core.design-icon")
        .unwrap()
        .is_empty());
    assert_eq!(
        indexer::get_module_dependents(&conn, "core.design_icon").unwrap(),
        vec![("app".to_string(), "app".to_string(), "api".to_string())]
    );
}

#[test]
fn exact_name_does_not_override_an_ambiguous_generated_accessor() {
    let project = TempDir::new().unwrap();
    let root = project.path();
    let files = vec![
        write_module(root, "core/designIcon", ""),
        write_module(root, "core/design-icon", ""),
        write_module(
            root,
            "app",
            r#"
                dependencies {
                    implementation(projects.core.designIcon)
                }
            "#,
        ),
    ];

    let mut conn = fresh_db();
    indexer::index_modules_from_files(&conn, root, &files).unwrap();
    let indexed = indexer::index_module_dependencies(&mut conn, root, &files, false).unwrap();

    assert_eq!(indexed, 0);
    assert!(indexer::get_module_deps(&conn, "app").unwrap().is_empty());
    assert!(indexer::get_module_dependents(&conn, "core.designIcon")
        .unwrap()
        .is_empty());
    assert!(indexer::get_module_dependents(&conn, "core.design-icon")
        .unwrap()
        .is_empty());
}
