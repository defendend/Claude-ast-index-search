//! Tests for `ast_index::indexer::detect_stacks` — the polyglot/KMP detector
//! that backs the smart `/initialize` command.

use std::fs;

use ast_index::indexer::detect_stacks;
use tempfile::TempDir;

fn kinds(detection: &ast_index::indexer::StackDetection) -> Vec<&str> {
    detection.stacks.iter().map(|s| s.kind.as_str()).collect()
}

#[test]
fn empty_directory_detects_no_stacks() {
    let tmp = TempDir::new().unwrap();
    let result = detect_stacks(tmp.path());
    assert!(result.stacks.is_empty());
    assert!(!result.is_kmp);
    assert!(!result.is_polyglot);
    assert!(!result.scan_truncated);
}

#[test]
fn rust_single_stack() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0\"\n",
    )
    .unwrap();

    let result = detect_stacks(tmp.path());
    assert_eq!(kinds(&result), vec!["rust"]);
    assert!(!result.is_kmp);
    assert!(!result.is_polyglot);
    assert_eq!(result.stacks[0].markers, vec!["Cargo.toml".to_string()]);
}

#[test]
fn web_plus_rust_is_polyglot() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0\"\n",
    )
    .unwrap();
    fs::write(tmp.path().join("package.json"), "{}").unwrap();
    fs::write(tmp.path().join("tsconfig.json"), "{}").unwrap();

    let result = detect_stacks(tmp.path());
    let ks = kinds(&result);
    assert!(ks.contains(&"rust"));
    assert!(ks.contains(&"web"));
    assert!(result.is_polyglot);
    assert!(!result.is_kmp);
}

#[test]
fn kmp_detected_when_plugin_and_source_set_present() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(
        root.join("settings.gradle.kts"),
        r#"rootProject.name = "demo""#,
    )
    .unwrap();
    fs::create_dir_all(root.join("composeApp/src/commonMain/kotlin")).unwrap();
    fs::create_dir_all(root.join("composeApp/src/androidMain/kotlin")).unwrap();
    fs::create_dir_all(root.join("composeApp/src/iosMain/kotlin")).unwrap();
    fs::write(
        root.join("composeApp/build.gradle.kts"),
        r#"
plugins {
    kotlin("multiplatform") version "1.9.0"
}
"#,
    )
    .unwrap();

    let result = detect_stacks(root);
    assert!(result.is_kmp, "expected KMP, got {:?}", result);
    let ks = kinds(&result);
    assert!(ks.contains(&"kmp"));
    assert!(ks.contains(&"android")); // Gradle marker is also there
                                      // KMP repo with only android + ios + kmp is NOT polyglot — it's KMP.
    assert!(!result.is_polyglot);
}

#[test]
fn kmp_plus_web_is_polyglot() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(
        root.join("settings.gradle.kts"),
        "rootProject.name = \"demo\"",
    )
    .unwrap();
    fs::create_dir_all(root.join("shared/src/commonMain/kotlin")).unwrap();
    fs::write(
        root.join("shared/build.gradle.kts"),
        "plugins { kotlin(\"multiplatform\") }",
    )
    .unwrap();
    fs::write(root.join("package.json"), "{}").unwrap();

    let result = detect_stacks(root);
    assert!(result.is_kmp);
    assert!(result.is_polyglot);
    let ks = kinds(&result);
    assert!(ks.contains(&"web"));
}

#[test]
fn android_alone_does_not_become_kmp() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(
        root.join("settings.gradle.kts"),
        r#"rootProject.name = "android-only""#,
    )
    .unwrap();
    fs::write(
        root.join("build.gradle.kts"),
        // Has Kotlin but NOT multiplatform — must not be classified as KMP.
        r#"plugins { id("org.jetbrains.kotlin.android") version "1.9.0" }"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("app/src/main/kotlin")).unwrap();

    let result = detect_stacks(root);
    assert!(!result.is_kmp);
    let ks = kinds(&result);
    assert!(ks.contains(&"android"));
    assert!(!ks.contains(&"kmp"));
}

#[test]
fn ios_detection_picks_up_xcodeproj_and_package_swift() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(root.join("Package.swift"), "// swift-tools-version:5.0").unwrap();
    fs::create_dir_all(root.join("MyApp.xcodeproj")).unwrap();

    let result = detect_stacks(root);
    let ks = kinds(&result);
    assert_eq!(ks, vec!["ios"]);
    let markers = &result.stacks[0].markers;
    assert!(markers.iter().any(|m| m == "Package.swift"));
    assert!(markers.iter().any(|m| m == "MyApp.xcodeproj"));
}

#[test]
fn nested_monorepo_stacks_are_detected_in_stable_order() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("apps/android")).unwrap();
    fs::create_dir_all(root.join("packages/web")).unwrap();
    fs::create_dir_all(root.join("crates/core")).unwrap();
    fs::write(root.join("apps/android/settings.gradle.kts"), "").unwrap();
    fs::write(root.join("packages/web/package.json"), "{}").unwrap();
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["android", "web", "rust"]);
    assert!(result.is_polyglot);
    assert!(!result.scan_truncated);
    assert_eq!(
        result.stacks[0].markers,
        vec!["apps/android/settings.gradle.kts"]
    );
    assert_eq!(result.stacks[1].markers, vec!["packages/web/package.json"]);
    assert_eq!(result.stacks[2].markers, vec!["crates/core/Cargo.toml"]);
}

#[test]
fn generated_dependency_directories_are_not_scanned_for_markers() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("node_modules/transitive-package")).unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("vendor/ignored-package")).unwrap();
    fs::write(root.join(".gitignore"), "vendor/\n").unwrap();
    fs::write(
        root.join("node_modules/transitive-package/package.json"),
        "{}",
    )
    .unwrap();
    fs::write(root.join("vendor/ignored-package/package.json"), "{}").unwrap();

    let result = detect_stacks(root);

    assert!(result.stacks.is_empty(), "unexpected stacks: {result:?}");
    assert!(!result.is_polyglot);
}

#[test]
fn kmp_plugin_and_source_set_in_different_gradle_modules_are_not_combined() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("plugin-module")).unwrap();
    fs::create_dir_all(root.join("source-module/src/commonMain/kotlin")).unwrap();
    fs::write(
        root.join("plugin-module/build.gradle.kts"),
        "plugins { kotlin(\"multiplatform\") }",
    )
    .unwrap();
    fs::write(root.join("source-module/build.gradle.kts"), "").unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["android"]);
    assert!(!result.is_kmp, "cross-module markers must be ambiguous");
    assert!(!result.is_polyglot);
}

#[test]
fn requirements_based_python_project_is_detected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Typical pip-tools / pyenv project without pyproject.toml or setup.py.
    fs::write(root.join("requirements.txt"), "mongoengine>=0.27\n").unwrap();
    fs::write(root.join("pytest.ini"), "[pytest]\n").unwrap();
    fs::write(root.join(".python-version"), "3.11\n").unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["python"]);
    assert_eq!(
        result.stacks[0].markers,
        vec!["requirements.txt".to_string(), "pytest.ini".to_string()]
    );
    assert!(!result.is_polyglot);
    assert_eq!(ast_index::indexer::project_label(root), "Python");
}

#[test]
fn pipenv_and_tox_markers_are_detected_as_python() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(root.join("Pipfile"), "[packages]\nrequests = \"*\"\n").unwrap();
    fs::write(root.join("Pipfile.lock"), "{}").unwrap();
    fs::write(root.join("tox.ini"), "[tox]\nenvlist = py311\n").unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["python"]);
    assert_eq!(
        result.stacks[0].markers,
        vec![
            "Pipfile".to_string(),
            "Pipfile.lock".to_string(),
            "tox.ini".to_string(),
        ]
    );
}

#[test]
fn pyproject_python_project_keeps_legacy_marker_order() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"x\"\nversion = \"0\"\n",
    )
    .unwrap();
    fs::write(root.join("requirements.txt"), "requests\n").unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["python"]);
    assert_eq!(
        result.stacks[0].markers,
        vec!["pyproject.toml".to_string(), "requirements.txt".to_string()]
    );
}

#[test]
fn nested_requirements_do_not_add_python_to_another_stack() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(root.join("settings.gradle.kts"), "rootProject.name = \"app\"\n").unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/requirements.txt"), "mkdocs\n").unwrap();
    fs::write(root.join("docs/tox.ini"), "[tox]\n").unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["android"]);
    assert!(!result.is_polyglot);
    assert_eq!(
        ast_index::indexer::detect_project_type(&root.join("docs")),
        ast_index::indexer::ProjectType::Unknown,
        "a docs dir with only requirements.txt must not count as a sub-project"
    );
}

#[test]
fn python_version_pin_alone_is_not_a_python_stack() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(root.join("package.json"), "{\"name\": \"web\"}\n").unwrap();
    fs::write(root.join(".python-version"), "3.11\n").unwrap();

    let result = detect_stacks(root);

    assert!(!kinds(&result).contains(&"python"));
    assert!(!result.is_polyglot);
}

#[test]
fn nested_pyproject_still_marks_a_python_stack() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("services/api")).unwrap();
    fs::write(root.join("services/api/pyproject.toml"), "[project]\nname = \"api\"\n").unwrap();
    fs::write(root.join("services/api/requirements.txt"), "fastapi\n").unwrap();

    let result = detect_stacks(root);

    assert_eq!(kinds(&result), vec!["python"]);
    assert_eq!(
        result.stacks[0].markers,
        vec!["services/api/pyproject.toml".to_string()]
    );
}
