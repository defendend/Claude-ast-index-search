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
