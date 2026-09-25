//! `explore` relevance and context on a small Rails-shaped project.
//!
//! The fixture reproduces what buried the answer on a real Rails app: words
//! of the query (`application`, `applicant`) that dozens of other names
//! carry, a class whose CamelCase name the full-text index keeps as one token
//! (`ApplicationService`, `PdfToHtmlService`) while snake_case names supply
//! enough exact `service` / `pdf` / `html` tokens that no fuzzy fallback runs,
//! associations and scopes that repeat the query's words, and a namespace
//! module opened in every file of a directory.

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

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fixture() -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = project.path();
    write(root, "Gemfile", "source 'https://rubygems.org'\n");

    for action in [
        "find",
        "select",
        "update",
        "create",
        "destroy",
        "find_all",
        "find_each",
        "find_by_uid",
        "select_statuses",
        "select_with_connection",
        "count",
        "archive",
        "restore",
        "import",
        "export",
        "publish",
        "unpublish",
        "copy",
        "move",
        "merge",
        "split",
        "lock",
        "unlock",
        "close",
        "reopen",
        "approve",
        "reject",
        "sync",
        "validate",
        "notify",
    ] {
        let class = action
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                let first = chars.next().unwrap().to_ascii_uppercase();
                format!("{first}{}", chars.as_str())
            })
            .collect::<String>();
        write(
            root,
            &format!("app/adapters/integrations/application/{action}_adapter.rb"),
            &format!(
                "module Integrations\n  module Application\n    class {class}Adapter\n      def call(application_id)\n        application_id\n      end\n    end\n  end\nend\n"
            ),
        );
    }

    let fields: String = (0..60)
        .map(|n| format!("  def applicant_field_{n}\n    {n}\n  end\n\n"))
        .collect();
    write(
        root,
        "app/models/applicant.rb",
        &format!("class Applicant\n{fields}end\n"),
    );
    write(
        root,
        "app/models/document.rb",
        r#"class Document
  def pdf_path; end
  def pdf_name; end
  def pdf_size; end
  def html_body; end
  def html_title; end
  def html_size; end
  def service_url; end
  def service_name; end
  def service_token; end
end
"#,
    );

    write(
        root,
        "app/services/application_service.rb",
        "class ApplicationService\n  def self.call(**params)\n    new(**params).tap(&:process)\n  end\nend\n",
    );
    write(
        root,
        "app/models/applicant/deduplication.rb",
        r#"module Applicant::Deduplication
  extend ActiveSupport::Concern

  included do
    has_many :applicant_merges
    has_one :applicant_merge_target
    scope :for_applicant_merge, -> { where(merge: true) }
  end
end
"#,
    );
    write(
        root,
        "app/services/applicant/merge_service.rb",
        r#"class Applicant::MergeService < ApplicationService
  def process
    merge_data
    save
  end

  private

  def merge_data
    @to.assign_attributes(@from.attributes)
  end

  def save
    @to.save!
  end
end
"#,
    );
    for helper in ["callbacks", "events", "create_event"] {
        let class = match helper {
            "callbacks" => "CallbacksService",
            "events" => "EventsService",
            _ => "CreateEventService",
        };
        write(
            root,
            &format!("app/services/applicant/merge/{helper}_service.rb"),
            &format!(
                "module Applicant::Merge\n  class {class} < ApplicationService\n    def process\n      true\n    end\n  end\nend\n"
            ),
        );
    }
    write(
        root,
        "app/services/applicant/cv_convert/pdf_to_html_service.rb",
        "class Applicant::CvConvert::PdfToHtmlService < ApplicationService\n  def process\n    convert\n  end\n\n  def convert\n    true\n  end\nend\n",
    );
    write(
        root,
        "app/services/offer/pdf_service.rb",
        "class Offer::PdfService < ApplicationService\n  def process\n    html\n  end\n\n  def html\n    true\n  end\nend\n",
    );
    write(
        root,
        "app/controllers/merges_controller.rb",
        "class MergesController\n  def update\n    Applicant::MergeService.call(to: 1, from: 2)\n  end\nend\n",
    );

    let rebuild = run(root, cache.path(), &["rebuild"]);
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    (project, cache)
}

fn explore_json(project: &TempDir, cache: &TempDir, query: &str) -> Value {
    let out = run(
        project.path(),
        cache.path(),
        &["--format", "json", "explore", query],
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|err| panic!("{err}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn symbol_names(doc: &Value) -> Vec<String> {
    doc["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn class_the_query_spells_out_leads_despite_a_common_word() {
    let (project, cache) = fixture();
    let names = symbol_names(&explore_json(&project, &cache, "application service"));
    assert_eq!(names[0], "ApplicationService", "{names:?}");
}

#[test]
fn camel_case_class_is_found_from_its_words() {
    let (project, cache) = fixture();
    let names = symbol_names(&explore_json(&project, &cache, "pdf to html service"));
    assert_eq!(
        names[0], "Applicant::CvConvert::PdfToHtmlService",
        "{names:?}"
    );
}

#[test]
fn question_leads_with_the_service_not_associations_or_namespace_modules() {
    let (project, cache) = fixture();
    let names = symbol_names(&explore_json(
        &project,
        &cache,
        "how does applicant merge work",
    ));
    assert_eq!(names[0], "Applicant::MergeService", "{names:?}");
    let top: Vec<&String> = names.iter().take(4).collect();
    assert!(
        top.iter()
            .all(|name| !name.contains(' ') && name.as_str() != "Applicant::Merge"),
        "{top:?}"
    );
}

#[test]
fn class_comes_with_an_outline_and_method_with_its_source() {
    let (project, cache) = fixture();
    let doc = explore_json(&project, &cache, "applicant merge service");
    let file = &doc["files"][0];
    assert_eq!(file["symbol"], "Applicant::MergeService", "{doc}");
    assert!(file.get("source").is_none(), "{file}");
    let rows = file["outline"].as_array().unwrap();
    let process = rows
        .iter()
        .find(|row| row["name"] == "process")
        .unwrap_or_else(|| panic!("{file}"));
    assert_eq!(process["kind"], "function");
    assert_eq!(process["line"], 2);
    assert_eq!(process["end_line"], 5);
    assert_eq!(file["outline_hidden"], 0);

    let doc = explore_json(&project, &cache, "merge_data");
    let file = &doc["files"][0];
    assert_eq!(file["symbol"], "merge_data", "{doc}");
    assert!(file.get("outline").is_none(), "{file}");
    assert!(
        file["source"].as_str().unwrap().contains("def merge_data"),
        "{file}"
    );
}

#[test]
fn text_outline_marks_the_chosen_definition() {
    let (project, cache) = fixture();
    let out = run(
        project.path(),
        cache.path(),
        &["explore", "applicant merge service"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(concat!(
            "#### app/services/applicant/merge_service.rb — Applicant::MergeService\n",
            "  → :1-16 Applicant::MergeService [class]\n",
            "    :2-5 process [function]\n",
        )),
        "{text}"
    );
}

#[test]
fn declaration_is_outlined_within_the_module_holding_it() {
    let (project, cache) = fixture();
    let doc = explore_json(&project, &cache, "applicant merge target");
    let file = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "app/models/applicant/deduplication.rb")
        .unwrap_or_else(|| panic!("{doc}"))
        .clone();
    let names: Vec<&str> = file["outline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(names[0], "Applicant::Deduplication", "{file}");
    assert!(
        names.contains(&"has_one :applicant_merge_target"),
        "{names:?}"
    );
}

fn neighbours(doc: &Value) -> Vec<(String, String)> {
    doc["neighbours"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["link"].as_str().unwrap().to_string(),
                format!(
                    "{}:{}",
                    n["path"].as_str().unwrap(),
                    n["name"].as_str().unwrap()
                ),
            )
        })
        .collect()
}

#[test]
fn rwr_takes_callers_from_the_symbol_graph_once_it_is_built() {
    let (project, cache) = fixture();
    let rwr = |project: &TempDir, cache: &TempDir| {
        let out = run(
            project.path(),
            cache.path(),
            &[
                "--format",
                "json",
                "explore",
                "applicant merge service",
                "--rwr",
            ],
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        neighbours(&doc)
    };
    let caller = (
        "caller".to_string(),
        "app/controllers/merges_controller.rb:update".to_string(),
    );

    // Without a graph neighbours come from references matched by name.
    let without_graph = rwr(&project, &cache);
    assert!(!without_graph.is_empty());

    let build = run(project.path(), cache.path(), &["graph", "build"]);
    assert!(build.status.success());
    let with_graph = rwr(&project, &cache);
    assert!(with_graph.contains(&caller), "{with_graph:?}");
}

#[test]
fn graph_dependents_leave_out_references_to_a_namesake_in_another_namespace() {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = project.path();
    write(root, "Gemfile", "source 'https://rubygems.org'\n");
    write(root, "app/models/invoice.rb", "class Invoice\nend\n");
    write(
        root,
        "app/services/charge.rb",
        "class Charge\n  def call\n    Invoice.new\n  end\nend\n",
    );
    write(
        root,
        "lib/archive/reader.rb",
        "module Archive\n  class Invoice\n  end\n\n  class Reader\n    def read\n      Invoice.new\n    end\n  end\nend\n",
    );
    assert!(run(root, cache.path(), &["rebuild"]).status.success());
    assert!(run(root, cache.path(), &["graph", "build"])
        .status
        .success());

    let db_path = run(root, cache.path(), &["db-path"]);
    let conn =
        rusqlite::Connection::open(String::from_utf8(db_path.stdout).unwrap().trim()).unwrap();
    let seed = ast_index::db::search_symbols(&conn, "Invoice", 10)
        .unwrap()
        .into_iter()
        .find(|s| s.path == "app/models/invoice.rb")
        .unwrap();
    let dependents = ast_index::commands::graph::resolved_dependents_of(&conn, &[seed], 10)
        .unwrap()
        .expect("graph is built and fresh");
    let paths: Vec<&str> = dependents[0].iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, vec!["app/services/charge.rb"], "{paths:?}");
}
