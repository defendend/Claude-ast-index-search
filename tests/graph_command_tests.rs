//! `graph` subcommands: build, resolution confidence, queries and staleness.
//!
//! Every test indexes its own small Ruby + TypeScript project through the real
//! binary, so resolution runs against parser output rather than hand-written
//! rows.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct Workspace {
    _temp: TempDir,
    root: PathBuf,
    db: PathBuf,
    cache: PathBuf,
}

fn workspace() -> Workspace {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    let db = temp.path().join("index.db");
    let cache = temp.path().join("cache");
    Workspace {
        _temp: temp,
        root,
        db,
        cache,
    }
}

impl Workspace {
    fn write(&self, path: &str, contents: &str) {
        let target = self.root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    fn ast_index(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&self.root)
            .env("AST_INDEX_DB_PATH", &self.db)
            .env("AST_INDEX_CACHE_DIR", &self.cache)
            .args(args)
            .output()
            .expect("ast-index must run")
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.ast_index(args);
        assert_success(&output);
        String::from_utf8(output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.extend_from_slice(&["--format", "json"]);
        let stdout = self.run(&all);
        serde_json::from_str(&stdout)
            .unwrap_or_else(|error| panic!("expected JSON from {args:?}: {error}; stdout={stdout}"))
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn items(report: &Value) -> &Vec<Value> {
    report["items"].as_array().expect("paginated report")
}

fn other_names(report: &Value) -> Vec<String> {
    items(report)
        .iter()
        .map(|item| item["other"]["name"].as_str().unwrap().to_string())
        .collect()
}

fn find_other<'a>(report: &'a Value, name: &str) -> &'a Value {
    items(report)
        .iter()
        .find(|item| item["other"]["name"] == name)
        .unwrap_or_else(|| panic!("no edge to/from {name} in {report:#}"))
}

/// A service layer with inheritance, a qualified call through a constant
/// receiver, a mixin cycle, an ambiguous member call, and a TypeScript module
/// that imports one of two same-named functions.
fn billing_project() -> Workspace {
    let ws = workspace();
    ws.write(
        "app/services/application_service.rb",
        r#"class ApplicationService
  def self.call(params = {})
    new(params).call
  end

  def call
    process
  end
end
"#,
    );
    ws.write(
        "app/models/application_record.rb",
        "class ApplicationRecord\nend\n",
    );
    ws.write(
        "app/models/invoice.rb",
        "class Invoice < ApplicationRecord\n  def total\n    1\n  end\nend\n",
    );
    ws.write(
        "app/services/billing/create_invoice_service.rb",
        r#"module Billing
  class CreateInvoiceService < ApplicationService
    def process
      Invoice.create!(amount: 1)
      Billing::Notifier.call(invoice: 1)
    end
  end
end
"#,
    );
    ws.write(
        "app/services/billing/notifier.rb",
        r#"module Billing
  class Notifier < ApplicationService
    def process
      deliver(1)
    end

    def deliver(message)
      message
    end
  end
end
"#,
    );
    ws.write(
        "app/controllers/invoices_controller.rb",
        r#"class InvoicesController
  def create
    Billing::CreateInvoiceService.call(params)
  end
end
"#,
    );
    ws.write(
        "lib/alpha.rb",
        "class Alpha\n  def refresh(value)\n    value\n  end\nend\n",
    );
    ws.write(
        "lib/beta.rb",
        "class Beta\n  def refresh(value)\n    value\n  end\nend\n",
    );
    ws.write(
        "lib/refresher.rb",
        "class Refresher\n  def run(item)\n    item.refresh(1)\n  end\nend\n",
    );
    ws.write("lib/ping.rb", "module Ping\n  include Pong\nend\n");
    ws.write("lib/pong.rb", "module Pong\n  include Ping\nend\n");
    ws.write(
        "web/api.ts",
        "export function fetchInvoices() {\n  return 1;\n}\n",
    );
    ws.write(
        "web/legacy.ts",
        "export function fetchInvoices() {\n  return 2;\n}\n",
    );
    ws.write(
        "web/List.tsx",
        r#"import { useState } from 'react';
import { fetchInvoices } from './api';

export function List() {
  useState(0);
  return fetchInvoices();
}
"#,
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws
}

#[test]
fn build_reports_edges_per_confidence_level() {
    let ws = billing_project();
    let summary = ws.json(&["graph", "build"]);
    assert!(summary["edges"].as_u64().unwrap() > 0);
    let level = |name: &str| -> u64 {
        summary["by_confidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["confidence"] == name)
            .unwrap()["edges"]
            .as_u64()
            .unwrap()
    };
    assert!(level("scoped") > 0, "{summary:#}");
    assert!(level("import") > 0, "{summary:#}");
    assert!(level("ambiguous") > 0, "{summary:#}");
    let status = ws.json(&["graph", "status"]);
    assert_eq!(status["graph"]["built"], true);
    assert_eq!(status["graph"]["stale"], false);
}

#[test]
fn queries_before_build_say_the_graph_is_missing() {
    let ws = billing_project();
    let text = ws.run(&["graph", "dependents", "Invoice"]);
    assert!(text.contains("graph build"), "{text}");
    let report = ws.json(&["graph", "dependents", "Invoice"]);
    assert_eq!(report["graph"]["built"], false);
    assert!(report["error"].is_string());
}

#[test]
fn dependents_resolve_superclasses_and_constant_receivers() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);

    let report = ws.json(&["graph", "dependents", "ApplicationService"]);
    let names = other_names(&report);
    assert!(
        names.contains(&"Billing::CreateInvoiceService".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"Billing::Notifier".to_string()),
        "{names:?}"
    );
    assert_eq!(
        find_other(&report, "Billing::Notifier")["confidence"],
        "scoped"
    );

    // `Billing::CreateInvoiceService.call(...)` reaches the inherited
    // singleton method, not the instance-level `call`.
    let singleton = ws.json(&[
        "graph",
        "dependents",
        "self.call",
        "--in-file",
        "application_service",
    ]);
    let create = find_other(&singleton, "create");
    assert_eq!(create["confidence"], "scoped");
    assert_eq!(create["subject"]["name"], "self.call");
}

#[test]
fn members_flag_covers_definitions_inside_a_class() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    let plain = ws.json(&["graph", "dependencies", "Billing::CreateInvoiceService"]);
    assert!(!other_names(&plain).contains(&"Invoice".to_string()));
    let with_members = ws.json(&[
        "graph",
        "dependencies",
        "Billing::CreateInvoiceService",
        "--members",
    ]);
    let invoice = find_other(&with_members, "Invoice");
    assert_eq!(invoice["confidence"], "scoped");
    assert_eq!(invoice["subject"]["name"], "process");
}

#[test]
fn ambiguous_edges_are_counted_but_listed_only_on_request() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    let hidden = ws.json(&["graph", "dependents", "refresh"]);
    assert_eq!(hidden["resolved_edges"], 0);
    assert_eq!(hidden["ambiguous_edges"], 2);
    assert!(items(&hidden).is_empty());

    let listed = ws.json(&["graph", "dependents", "refresh", "--include-ambiguous"]);
    assert_eq!(items(&listed).len(), 2);
    for item in items(&listed) {
        assert_eq!(item["confidence"], "ambiguous");
        assert_eq!(item["candidates"], 2);
        assert_eq!(item["other"]["name"], "run");
    }

    let metrics = ws.json(&["graph", "metrics", "Alpha#refresh"]);
    let row = &items(&metrics)[0];
    assert_eq!(row["fan_in"], 0);
    assert_eq!(row["fan_in_ambiguous"], 1);
}

#[test]
fn typescript_imports_pick_the_imported_definition() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "dependencies", "List"]);
    let fetch = find_other(&report, "fetchInvoices");
    assert_eq!(fetch["confidence"], "import");
    assert!(
        fetch["other"]["path"]
            .as_str()
            .unwrap()
            .ends_with("web/api.ts"),
        "{fetch:#}"
    );
    assert_eq!(items(&report).len(), 1, "useState comes from a package");
}

#[test]
fn impact_counts_transitive_dependents_per_depth() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "impact", "ApplicationRecord", "--depth", "2"]);
    let levels = report["levels"].as_array().unwrap();
    assert_eq!(levels[0]["depth"], 1);
    let affected: Vec<&str> = items(&report)
        .iter()
        .map(|item| item["symbol"]["name"].as_str().unwrap())
        .collect();
    assert!(affected.contains(&"Invoice"), "{affected:?}");
    assert!(affected.contains(&"process"), "{affected:?}");
    let invoice = items(&report)
        .iter()
        .find(|item| item["symbol"]["name"] == "Invoice")
        .unwrap();
    assert_eq!(invoice["depth"], 1);
    assert_eq!(invoice["via"], "ApplicationRecord");
}

#[test]
fn path_steps_into_class_members_and_marks_the_hop() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "path", "InvoicesController", "Invoice"]);
    assert_eq!(report["direction"], "forward");
    let hops: Vec<(&str, Option<&str>)> = items(&report)[0]
        .as_array()
        .unwrap()
        .iter()
        .map(|hop| {
            (
                hop["symbol"]["name"].as_str().unwrap(),
                hop["edge"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        hops,
        vec![
            ("create", Some("scoped")),
            ("Billing::CreateInvoiceService", Some("contains")),
            ("process", Some("scoped")),
            ("Invoice", None),
        ]
    );

    let reverse = ws.json(&["graph", "path", "Invoice", "InvoicesController"]);
    assert_eq!(reverse["direction"], "reverse");
}

#[test]
fn cycles_and_top_use_resolved_edges() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    let cycles = ws.json(&["graph", "cycles"]);
    let members: Vec<&str> = items(&cycles)
        .iter()
        .flat_map(|cycle| cycle["members"].as_array().unwrap())
        .map(|member| member["name"].as_str().unwrap())
        .collect();
    assert!(
        members.contains(&"Ping") && members.contains(&"Pong"),
        "{cycles:#}"
    );

    let top = ws.json(&["graph", "top", "--sort", "fan-in", "--limit", "3"]);
    let names: Vec<&str> = items(&top)
        .iter()
        .map(|item| item["symbol"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ApplicationService"), "{names:?}");
    assert!(items(&top)[0]["pagerank_pct"].as_f64().is_some());
}

#[test]
fn update_marks_the_graph_stale_until_it_is_rebuilt() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    ws.write(
        "app/services/billing/refund_service.rb",
        "module Billing\n  class RefundService < ApplicationService\n    def process\n      Invoice.find(1)\n    end\n  end\nend\n",
    );
    assert_success(&ws.ast_index(&["update"]));

    let status = ws.json(&["graph", "status"]);
    assert_eq!(status["graph"]["stale"], true);
    let text = ws.run(&["graph", "dependents", "Invoice", "--members"]);
    assert!(text.contains("stale"), "{text}");
    let stale = ws.json(&["graph", "dependents", "ApplicationService"]);
    assert_eq!(stale["graph"]["stale"], true);
    assert!(!other_names(&stale).contains(&"Billing::RefundService".to_string()));

    let fresh = ws.json(&["graph", "dependents", "ApplicationService", "--refresh"]);
    assert_eq!(fresh["graph"]["stale"], false);
    assert!(other_names(&fresh).contains(&"Billing::RefundService".to_string()));
}

#[test]
fn an_update_without_changes_keeps_the_graph_fresh() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    assert_success(&ws.ast_index(&["update"]));
    assert_eq!(ws.json(&["graph", "status"])["graph"]["stale"], false);
}

#[test]
fn rebuild_drops_the_graph() {
    let ws = billing_project();
    ws.run(&["graph", "build"]);
    assert_success(&ws.ast_index(&["rebuild"]));
    assert_eq!(ws.json(&["graph", "status"])["graph"]["built"], false);
}

#[test]
fn subtree_filters_are_rejected_for_build() {
    let ws = billing_project();
    let output = ws.ast_index(&["--local", "graph", "build"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("graph build"));
}

#[test]
fn scoped_constant_assignment_shadows_a_top_level_module() {
    let ws = workspace();
    ws.write(
        "app/services/import/base.rb",
        "module Import\n  def helper(value)\n    value\n  end\nend\n",
    );
    ws.write(
        "lib/billing/import.rb",
        "Billing::Import = Billing::Container.injector\n",
    );
    ws.write(
        "app/services/billing/charge.rb",
        r#"module Billing
  class Charge
    include Import[:repo]

    def run
      helper(1)
    end
  end
end
"#,
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws.run(&["graph", "build"]);

    let injector = ws.json(&["graph", "dependents", "Billing::Import"]);
    assert_eq!(
        find_other(&injector, "Billing::Charge")["confidence"],
        "scoped"
    );
    let module = ws.json(&["graph", "dependents", "::Import"]);
    assert!(items(&module).is_empty(), "{module:#}");
    // The injector is not a mixin, so the module's methods are not inherited.
    let helper = ws.json(&["graph", "dependents", "Import#helper"]);
    assert_eq!(helper["resolved_edges"], 0, "{helper:#}");
}

/// A Rails app whose `db/schema.rb` is gitignored, with models linked to
/// tables by convention, `self.table_name`, single-table inheritance and
/// nesting.
fn rails_schema_project() -> Workspace {
    let ws = workspace();
    ws.write(".gitignore", "db/schema.rb\n");
    fs::create_dir_all(ws.root.join(".git")).unwrap();
    ws.write(
        "db/schema.rb",
        r#"ActiveRecord::Schema[7.1].define(version: 2024_01_01_000000) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.boolean "archived", default: false
    t.string "type"
  end

  create_table "clients" do |t|
    t.string "first_name"
  end

  create_table "orders" do |t|
    t.string "number"
  end

  create_table "order_lines" do |t|
    t.integer "quantity"
  end

  create_table "audits" do |t|
    t.string "action"
  end
end
"#,
    );
    ws.write(
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    );
    ws.write(
        "app/models/person.rb",
        r#"class Person < ApplicationRecord
  def display_name
    first_name
  end

  def hidden
    archived? || self.first_name.nil?
  end

  def self.lookup(value)
    first_name
  end
end
"#,
    );
    ws.write(
        "app/models/admin.rb",
        "class Admin < Person\n  def label\n    first_name_changed? && first_name\n  end\nend\n",
    );
    ws.write(
        "app/models/customer.rb",
        "class Customer < ApplicationRecord\n  self.table_name = \"clients\"\n\n  def greeting\n    first_name\n  end\nend\n",
    );
    ws.write(
        "app/models/order.rb",
        "class Order < ApplicationRecord\n  class Line < ApplicationRecord\n    def total\n      quantity * 2\n    end\n  end\nend\n",
    );
    ws.write(
        "app/models/invoice.rb",
        "class Invoice < ApplicationRecord\nend\n",
    );
    ws.write(
        "app/services/greeter.rb",
        "class Greeter\n  def run(person)\n    person.first_name\n  end\nend\n",
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws
}

#[test]
fn schema_columns_are_indexed_even_when_the_dump_is_gitignored() {
    let ws = rails_schema_project();
    let columns = ws.run(&["search", "first_name", "-t", "column"]);
    assert!(columns.contains("people.first_name"), "{columns}");
    assert!(columns.contains("clients.first_name"), "{columns}");
    let outline = ws.run(&["outline", "db/schema.rb"]);
    assert!(outline.contains("people [table]"), "{outline}");
    assert!(outline.contains("people.archived [column]"), "{outline}");

    ws.write(
        "db/schema.rb",
        "ActiveRecord::Schema[7.1].define(version: 2) do\n  create_table \"people\" do |t|\n    t.string \"nickname\"\n  end\nend\n",
    );
    assert_success(&ws.ast_index(&["update"]));
    let updated = ws.run(&["search", "nickname", "-t", "column"]);
    assert!(updated.contains("people.nickname"), "{updated}");
    let gone = ws.run(&["search", "archived", "-t", "column"]);
    assert!(!gone.contains("people.archived"), "{gone}");
}

#[test]
fn model_code_resolves_to_the_columns_of_its_table() {
    let ws = rails_schema_project();
    let summary = ws.json(&["graph", "build"]);
    let schema = &summary["schema"];
    assert_eq!(schema["tables"], 5, "{schema:#}");
    assert_eq!(schema["columns"], 7, "{schema:#}");
    assert_eq!(schema["by_rule"]["explicit"], 1, "{schema:#}");
    assert_eq!(schema["by_rule"]["inherited"], 1, "{schema:#}");
    assert_eq!(schema["by_rule"]["nested"], 1, "{schema:#}");
    assert_eq!(schema["by_rule"]["convention"], 2, "{schema:#}");
    assert!(schema["by_rule"].get("prefixed").is_none(), "{schema:#}");
    assert_eq!(schema["tables_without_model"][0], "audits");
    assert_eq!(schema["models_without_table"][0]["model"], "Invoice");
    assert_eq!(schema["models_without_table"][0]["table"], "invoices");

    let people = ws.json(&["graph", "dependents", "people.first_name"]);
    let mut sources = other_names(&people);
    sources.sort();
    // Readers and attribute methods in the model and its STI subclass; not
    // the class method, not a call on another receiver, not the other table.
    assert_eq!(
        sources,
        vec!["display_name", "hidden", "label"],
        "{people:#}"
    );
    for item in items(&people) {
        assert_eq!(item["confidence"], "scoped");
    }
    let archived = ws.json(&["graph", "dependents", "people#archived"]);
    assert_eq!(other_names(&archived), vec!["hidden"], "{archived:#}");
    let clients = ws.json(&["graph", "dependents", "clients.first_name"]);
    assert_eq!(other_names(&clients), vec!["greeting"], "{clients:#}");
    let lines = ws.json(&["graph", "dependents", "order_lines.quantity"]);
    assert_eq!(other_names(&lines), vec!["total"], "{lines:#}");
}

#[test]
fn production_code_never_resolves_into_test_trees() {
    let ws = workspace();
    ws.write(
        "app/workers/application_worker.rb",
        "class ApplicationWorker\nend\n",
    );
    ws.write(
        "app/workers/sync_worker.rb",
        "class SyncWorker < ApplicationWorker\nend\n",
    );
    ws.write(
        "spec/support/stubs.rb",
        "class ApplicationWorker\n  def self.enqueue(*args)\n    args\n  end\nend\n",
    );
    ws.write(
        "app/services/sync_service.rb",
        "class SyncService\n  def run\n    SyncWorker.enqueue(1)\n  end\nend\n",
    );
    ws.write(
        "spec/services/sync_service_spec.rb",
        "class SyncServiceProbe\n  def run\n    SyncWorker.enqueue(2)\n  end\nend\n",
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "dependents", "self.enqueue"]);
    let names = other_names(&report);
    assert_eq!(names, vec!["run"], "{report:#}");
    assert!(items(&report)[0]["other"]["path"]
        .as_str()
        .unwrap()
        .starts_with("spec/"));
}

#[test]
fn a_ruby_namespace_named_tests_is_production_code() {
    let ws = workspace();
    ws.write(
        "app/models/tests/grader.rb",
        "module Tests\n  class Grader\n    def self.grade(answers)\n      answers\n    end\n  end\nend\n",
    );
    ws.write(
        "app/services/scoring_service.rb",
        "class ScoringService\n  def run\n    Tests::Grader.grade([])\n  end\nend\n",
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "dependents", "self.grade"]);
    assert_eq!(other_names(&report), vec!["run"], "{report:#}");

    let top = ws.json(&["graph", "top", "--exclude-tests"]);
    assert!(
        items(&top)
            .iter()
            .any(|item| item["symbol"]["path"] == "app/models/tests/grader.rb"),
        "{top:#}"
    );
}

#[test]
fn ruby_calls_without_parentheses_are_references() {
    let ws = workspace();
    ws.write(
        "app/services/report.rb",
        r#"class Report
  def build(rows)
    total = 0
    rows.each { |row| total += row.amount }
    header_line
    self.footer_line
    total.to_s
  end

  def header_line
    1
  end

  def footer_line
    2
  end
end
"#,
    );
    ws.write(
        "spec/services/report_spec.rb",
        r#"RSpec.describe Report do
  let(:report) { Report.new }

  it "builds" do
    expect(report.build([])).to eq("0")
  end
end
"#,
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    let usages = ws.run(&["usages", "header_line"]);
    assert!(usages.contains("app/services/report.rb:5"), "{usages}");
    let footer = ws.run(&["usages", "footer_line"]);
    assert!(footer.contains("app/services/report.rb:6"), "{footer}");
    for local in ["total", "row", "rows"] {
        let text = ws.run(&["usages", local]);
        assert!(!text.contains("report.rb"), "{local} is a local: {text}");
    }

    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "dependencies", "build"]);
    let mut names = other_names(&report);
    names.sort();
    assert_eq!(names, vec!["footer_line", "header_line"], "{report:#}");
    let helper = ws.json(&["graph", "dependents", "report", "--in-file", "report_spec"]);
    assert_eq!(items(&helper).len(), 1, "{helper:#}");
    let example = find_other(&helper, "it \"builds\"");
    assert_eq!(example["confidence"], "local");
}

#[test]
fn a_project_vendor_directory_is_part_of_the_graph() {
    let ws = workspace();
    ws.write(
        "vendor/billing_sdk/client.rb",
        "class BillingClient\n  def self.charge(amount)\n    amount\n  end\nend\n",
    );
    ws.write(
        "app/services/checkout.rb",
        "class Checkout\n  def run\n    BillingClient.charge(1)\n  end\nend\n",
    );
    ws.write(
        "node_modules/billing-sdk/index.d.ts",
        "export declare class BillingClient {\n  static charge(amount: number): number;\n}\n",
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws.run(&["graph", "build"]);
    let report = ws.json(&["graph", "dependents", "BillingClient"]);
    let matched: Vec<&str> = report["matched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["path"].as_str().unwrap())
        .collect();
    assert_eq!(matched, vec!["vendor/billing_sdk/client.rb"], "{report:#}");
    let charge = ws.json(&["graph", "dependents", "BillingClient#charge"]);
    assert_eq!(other_names(&charge), vec!["run"], "{charge:#}");
}

/// A Rust crate whose calls reach other modules through their paths, `use`
/// bindings, a re-export and the library's name from an integration test.
/// `legacy` defines a second `open_db`, and a `HashMap` type with a `new`.
fn rust_crate_project() -> Workspace {
    let ws = workspace();
    ws.write(
        "Cargo.toml",
        "[package]\nname = \"demo-app\"\nversion = \"0.1.0\"\n",
    );
    ws.write(
        "src/lib.rs",
        "pub mod commands;\npub mod db;\npub mod legacy;\n",
    );
    ws.write(
        "src/db.rs",
        concat!(
            "pub struct Scope;\n\n",
            "impl Scope {\n",
            "    pub fn none() -> Scope {\n",
            "        Scope\n",
            "    }\n",
            "}\n\n",
            "pub fn open_db() -> u8 {\n",
            "    1\n",
            "}\n",
        ),
    );
    ws.write(
        "src/legacy.rs",
        concat!(
            "pub struct HashMap;\n\n",
            "impl HashMap {\n",
            "    pub fn new() -> HashMap {\n",
            "        HashMap\n",
            "    }\n",
            "}\n\n",
            "pub fn open_db() -> u8 {\n",
            "    2\n",
            "}\n",
        ),
    );
    ws.write(
        "src/commands/mod.rs",
        concat!(
            "pub mod grep;\n",
            "mod paths;\n\n",
            "pub use paths::is_test_path;\n\n",
            "pub fn helper() -> u8 {\n",
            "    3\n",
            "}\n",
        ),
    );
    ws.write(
        "src/commands/paths.rs",
        concat!(
            "pub fn is_test_path(path: &str) -> bool {\n",
            "    path.is_empty()\n",
            "}\n\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    use super::*;\n\n",
            "    #[test]\n",
            "    fn detects() {\n",
            "        assert!(is_test_path(\"\"));\n",
            "    }\n",
            "}\n",
        ),
    );
    ws.write(
        "src/commands/grep.rs",
        concat!(
            "use std::collections::HashMap;\n\n",
            "use super::is_test_path;\n",
            "use crate::db::{self, Scope as DbScope};\n\n",
            "pub fn run() -> usize {\n",
            "    let conn = db::open_db();\n",
            "    let scope = DbScope::none();\n",
            "    let helped = super::helper();\n",
            "    let map: HashMap<u8, u8> = HashMap::new();\n",
            "    usize::from(is_test_path(\"x\")) + map.len() + usize::from(conn + helped)\n",
            "}\n",
        ),
    );
    ws.write(
        "tests/api.rs",
        concat!(
            "use demo_app::db;\n\n",
            "#[test]\n",
            "fn opens() {\n",
            "    assert_eq!(db::open_db(), 1);\n",
            "}\n",
        ),
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws.run(&["graph", "build"]);
    ws
}

#[test]
fn rust_paths_resolve_through_file_modules_and_use_declarations() {
    let ws = rust_crate_project();
    let run = ws.json(&["graph", "dependencies", "run"]);
    let target = |name: &str| -> (String, String) {
        let edge = find_other(&run, name);
        (
            edge["other"]["path"].as_str().unwrap().to_string(),
            edge["confidence"].as_str().unwrap().to_string(),
        )
    };
    assert_eq!(target("open_db"), ("src/db.rs".into(), "scoped".into()));
    assert_eq!(target("none"), ("src/db.rs".into(), "scoped".into()));
    assert_eq!(
        target("helper"),
        ("src/commands/mod.rs".into(), "scoped".into())
    );
    assert_eq!(
        target("is_test_path"),
        ("src/commands/paths.rs".into(), "import".into())
    );
    assert!(
        !other_names(&run).contains(&"new".to_string()),
        "std's HashMap::new is not the project's: {run:#}"
    );

    let open_db = ws.json(&["graph", "dependents", "open_db", "--in-file", "src/db.rs"]);
    let mut callers = other_names(&open_db);
    callers.sort();
    assert_eq!(callers, vec!["opens", "run"], "{open_db:#}");
    let legacy = ws.json(&["graph", "dependents", "open_db", "--in-file", "legacy"]);
    assert!(items(&legacy).is_empty(), "{legacy:#}");

    let detects = ws.json(&["graph", "dependencies", "detects"]);
    assert_eq!(find_other(&detects, "is_test_path")["confidence"], "local");
}

/// `MergeService` is called from one production method and from specs, and
/// that method is itself exercised by a spec.
fn merge_service_project() -> Workspace {
    let ws = workspace();
    ws.write(
        "app/services/merge_service.rb",
        "class MergeService\n  def self.call\n    1\n  end\nend\n",
    );
    ws.write(
        "app/services/dedupe.rb",
        "class Dedupe\n  def run\n    MergeService.call\n  end\nend\n",
    );
    ws.write(
        "spec/services/merge_service_spec.rb",
        "describe MergeService do\n  it \"merges\" do\n    MergeService.call\n  end\nend\n",
    );
    ws.write(
        "spec/services/dedupe_spec.rb",
        "describe Dedupe do\n  it \"runs\" do\n    Dedupe.new.run\n  end\nend\n",
    );
    assert_success(&ws.ast_index(&["rebuild"]));
    ws.run(&["graph", "build"]);
    ws
}

#[test]
fn exclude_tests_leaves_test_dependents_out_of_dependents_and_impact() {
    let ws = merge_service_project();
    let all = ws.json(&["graph", "dependents", "MergeService"]);
    assert!(
        items(&all)
            .iter()
            .any(|item| item["other"]["path"].as_str().unwrap().starts_with("spec/")),
        "{all:#}"
    );
    let production = ws.json(&["graph", "dependents", "MergeService", "--exclude-tests"]);
    assert_eq!(other_names(&production), vec!["run"], "{production:#}");
    assert_eq!(production["exclude_tests"], true);
    assert!(production["excluded_test_edges"].as_u64().unwrap() >= 1);
    assert_eq!(
        production["resolved_edges"].as_u64().unwrap() as usize,
        items(&production).len()
    );
    let text = ws.run(&["graph", "dependents", "MergeService", "--exclude-tests"]);
    assert!(text.contains("from test files left out"), "{text}");

    let impact = ws.json(&["graph", "impact", "MergeService", "--depth", "3"]);
    assert!(impact["total_symbols"].as_u64().unwrap() > 1, "{impact:#}");
    let impact = ws.json(&[
        "graph",
        "impact",
        "MergeService",
        "--depth",
        "3",
        "--exclude-tests",
    ]);
    let names: Vec<&str> = items(&impact)
        .iter()
        .map(|item| item["symbol"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["run"], "{impact:#}");
    assert_eq!(impact["total_symbols"], 1);
    assert!(
        impact["excluded_test_symbols"].as_u64().unwrap() >= 2,
        "{impact:#}"
    );
}
