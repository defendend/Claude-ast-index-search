//! Which subclasses `implementations` attributes to a parent type.
//!
//! A parent written through a namespace or package (`com.acme.Base`,
//! `acme::Base`, `::ApplicationService`) is the same type and must match; a
//! parent that is a different, separately defined type ending in the same
//! name (`Legacy::ApplicationService`) must not.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> Value {
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
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn project(files: &[(&str, &str)]) -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    for (rel, content) in files {
        let path = project.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    run(project.path(), cache.path(), &["rebuild"]);
    (project, cache)
}

fn implementations(project: &(TempDir, TempDir), parent: &str) -> (Vec<String>, u64) {
    let doc = run(
        project.0.path(),
        project.1.path(),
        &["--format", "json", "implementations", parent],
    );
    let names = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_string())
        .collect();
    (names, doc["pagination"]["total"].as_u64().unwrap())
}

fn ruby_project() -> (TempDir, TempDir) {
    project(&[
        ("Gemfile", "source 'https://rubygems.org'\n"),
        (
            "app/services/application_service.rb",
            "class ApplicationService\nend\n",
        ),
        (
            "app/services/legacy/application_service.rb",
            "class Legacy::ApplicationService\nend\n",
        ),
        (
            "app/services/charge_service.rb",
            "class ChargeService < ApplicationService\nend\n",
        ),
        (
            "app/services/refund_service.rb",
            "class RefundService < ::ApplicationService\nend\n",
        ),
        (
            "app/services/old_charge_service.rb",
            "class OldChargeService < Legacy::ApplicationService\nend\n",
        ),
        (
            "app/services/external_service.rb",
            "class ExternalService < Vendor::ApplicationService\nend\n",
        ),
    ])
}

#[test]
fn ruby_subclass_of_another_namespace_class_of_the_same_name_is_left_out() {
    let ruby = ruby_project();
    let (names, total) = implementations(&ruby, "ApplicationService");
    assert_eq!(total, names.len() as u64, "{names:?}");
    assert!(names.contains(&"ChargeService".to_string()), "{names:?}");
    assert!(
        !names.contains(&"OldChargeService".to_string()),
        "{names:?}"
    );
}

#[test]
fn ruby_top_level_constant_path_is_the_same_class_and_ranks_as_direct() {
    let ruby = ruby_project();
    let (names, _) = implementations(&ruby, "ApplicationService");
    let refund = names.iter().position(|n| n == "RefundService").unwrap();
    let external = names.iter().position(|n| n == "ExternalService").unwrap();
    assert!(refund < external, "{names:?}");
}

#[test]
fn ruby_parent_whose_namespace_the_index_does_not_define_still_matches() {
    let ruby = ruby_project();
    let (names, _) = implementations(&ruby, "ApplicationService");
    assert!(names.contains(&"ExternalService".to_string()), "{names:?}");
}

#[test]
fn ruby_qualified_parent_query_lists_its_own_subclasses() {
    let ruby = ruby_project();
    let (names, total) = implementations(&ruby, "Legacy::ApplicationService");
    assert_eq!(names, vec!["OldChargeService"]);
    assert_eq!(total, 1);
}

#[test]
fn java_parent_named_through_its_package_still_matches() {
    let java = project(&[
        (
            "src/main/java/com/acme/Base.java",
            "package com.acme;\n\npublic class Base {}\n",
        ),
        (
            "src/main/java/com/acme/app/Child.java",
            "package com.acme.app;\n\npublic class Child extends com.acme.Base {}\n",
        ),
        (
            "src/main/java/com/acme/app/Sibling.java",
            "package com.acme.app;\n\nimport com.acme.Base;\n\npublic class Sibling extends Base {}\n",
        ),
    ]);
    let (names, total) = implementations(&java, "Base");
    assert_eq!(total, 2, "{names:?}");
    assert!(names.contains(&"Child".to_string()), "{names:?}");
    assert!(names.contains(&"Sibling".to_string()), "{names:?}");
}

#[test]
fn cpp_parent_named_through_its_namespace_still_matches() {
    let cpp = project(&[
        ("src/base.hpp", "namespace acme {\nclass Base {};\n}\n"),
        (
            "src/child.hpp",
            "#include \"base.hpp\"\nclass Child : public acme::Base {};\n",
        ),
    ]);
    let (names, _) = implementations(&cpp, "Base");
    assert!(names.contains(&"Child".to_string()), "{names:?}");
}
