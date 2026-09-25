//! `search --rank`: presets re-order files and symbols by Git history and the
//! symbol graph, keep relevance tiers, refuse to rank without evidence, and
//! explain every position in both output formats.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
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
    fn ast_index(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(&self.root)
            .env("AST_INDEX_DB_PATH", &self.db)
            .env("AST_INDEX_CACHE_DIR", &self.cache)
            .env_remove("AST_INDEX_VCS_BIN")
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

    fn write(&self, path: &str, contents: &str) {
        let target = self.root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    /// Commit `files` with a fixed author and date so ages and recency are
    /// deterministic relative to each other.
    fn commit(&self, date: &str, author: &str, message: &str, files: &[(&str, &str)]) {
        for (path, contents) in files {
            self.write(path, contents);
            git_in(&self.root, &[], &["add", path]);
        }
        let email = format!("{author}@example.invalid");
        git_in(
            &self.root,
            &[
                ("GIT_AUTHOR_DATE", date),
                ("GIT_COMMITTER_DATE", date),
                ("GIT_AUTHOR_NAME", author),
                ("GIT_AUTHOR_EMAIL", &email),
            ],
            &["commit", "-q", "-m", message],
        );
    }
}

fn git_in(repo: &Path, env: &[(&str, &str)], args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Dev One")
        .env("GIT_AUTHOR_EMAIL", "dev@example.invalid")
        .env("GIT_COMMITTER_NAME", "Dev One")
        .env("GIT_COMMITTER_EMAIL", "dev@example.invalid")
        .envs(env.iter().copied())
        .args(args)
        .output()
        .expect("git must run");
    assert!(
        output.status.success(),
        "git {:?}: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

fn ruby_class(name: &str, body: &str) -> String {
    format!("class {name}\n  def call(amount)\n{body}    amount\n  end\nend\n")
}

/// A billing area where each preset has a different right answer. Graph
/// edges start at the calling method, so a class's dependents are the
/// methods that name it:
///
/// * `BillingRetry` — recent, rewritten by six bugfix commits, called from
///   three files (hotspot, risky, central);
/// * `BillingCore` — old, calm, called from two files (proven);
/// * `BillingLedger` — old, calm, called from one file (proven);
/// * `BillingDraft` — five bugfix commits, called from nowhere (hotspot, but
///   never risky: nothing depends on it);
/// * `Billing` — the exact name typed, unused: it must stay on top anyway.
fn billing_project() -> Workspace {
    let ws = workspace();
    git_in(&ws.root, &[], &["init", "-q", "-b", "main"]);
    ws.commit(
        "2019-01-01T10:00:00",
        "dev1",
        "Add billing namespace",
        &[("app/billing.rb", "module Billing\nend\n")],
    );
    ws.commit(
        "2019-01-02T10:00:00",
        "dev1",
        "Add billing core",
        &[("app/billing_core.rb", &ruby_class("BillingCore", ""))],
    );
    ws.commit(
        "2019-02-01T10:00:00",
        "dev2",
        "Add billing ledger",
        &[(
            "app/billing_ledger.rb",
            &ruby_class("BillingLedger", "    BillingCore.new.call(amount)\n"),
        )],
    );
    ws.commit(
        "2024-01-01T10:00:00",
        "dev1",
        "Add billing retry",
        &[(
            "app/billing_retry.rb",
            &ruby_class("BillingRetry", "    BillingCore.new.call(amount)\n"),
        )],
    );
    for round in 1..=6 {
        let body = format!("    BillingCore.new.call(amount) if {round} > 0\n");
        ws.commit(
            &format!("2026-0{round}-01T10:00:00"),
            &format!("dev{}", round % 3 + 1),
            &format!("Fix retry crash number {round}"),
            &[("app/billing_retry.rb", &ruby_class("BillingRetry", &body))],
        );
    }
    for round in 1..=5 {
        let body = format!("    amount if {round} > 0\n");
        ws.commit(
            &format!("2026-0{round}-15T10:00:00"),
            "dev2",
            &format!("Fix draft rounding bug {round}"),
            &[("app/billing_draft.rb", &ruby_class("BillingDraft", &body))],
        );
    }
    ws.commit(
        "2025-01-01T10:00:00",
        "dev3",
        "Add checkout",
        &[(
            "app/checkout.rb",
            &ruby_class(
                "Checkout",
                "    BillingRetry.new.call(amount)\n    BillingLedger.new.call(amount)\n",
            ),
        )],
    );
    ws.commit(
        "2025-02-01T10:00:00",
        "dev3",
        "Add invoices",
        &[(
            "app/invoices.rb",
            &ruby_class("Invoices", "    BillingRetry.new.call(amount)\n"),
        )],
    );
    ws.commit(
        "2025-03-01T10:00:00",
        "dev2",
        "Add reports",
        &[(
            "app/reports.rb",
            &ruby_class("Reports", "    BillingRetry.new.call(amount)\n"),
        )],
    );
    ws
}

fn collect_everything(ws: &Workspace) {
    ws.run(&["rebuild"]);
    ws.run(&["hotspots", "--collect"]);
    ws.run(&["graph", "build"]);
}

fn symbol_names(report: &Value) -> Vec<String> {
    report["symbols"]
        .as_array()
        .expect("symbols array")
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap().to_string())
        .collect()
}

fn symbol<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|symbol| symbol["name"] == name)
        .unwrap_or_else(|| panic!("no symbol {name} in {report:#}"))
}

fn position(names: &[String], name: &str) -> usize {
    names
        .iter()
        .position(|candidate| candidate == name)
        .unwrap_or_else(|| panic!("{name} missing from {names:?}"))
}

#[test]
fn search_without_rank_keeps_plain_relevance_output() {
    let ws = billing_project();
    collect_everything(&ws);

    let report = ws.json(&["search", "Billing"]);
    assert!(report.get("rank").is_none(), "no rank block without --rank");
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(Value::is_string),
        "files stay plain strings without --rank: {}",
        report["files"]
    );
    assert!(report["symbols"][0].get("rank").is_none());
    assert_eq!(
        symbol_names(&report),
        vec![
            "Billing",
            "BillingCore",
            "BillingDraft",
            "BillingRetry",
            "BillingLedger"
        ]
    );
    let text = ws.run(&["search", "Billing"]);
    assert!(!text.contains("ranked:"), "{text}");
}

#[test]
fn hotspots_preset_lifts_the_frequently_fixed_file() {
    let ws = billing_project();
    collect_everything(&ws);

    let names = symbol_names(&ws.json(&["search", "Billing", "--rank", "hotspots"]));
    assert_eq!(names[0], "Billing", "the exact name stays first");
    let mut hot = names[1..3].to_vec();
    hot.sort();
    assert_eq!(hot, vec!["BillingDraft", "BillingRetry"], "{names:?}");
}

#[test]
fn proven_preset_prefers_calm_used_code_over_the_hotspot() {
    let ws = billing_project();
    collect_everything(&ws);

    let report = ws.json(&["search", "Billing", "--rank", "proven"]);
    let names = symbol_names(&report);
    assert_eq!(names[0], "Billing");
    for calm in ["BillingCore", "BillingLedger"] {
        for hot in ["BillingRetry", "BillingDraft"] {
            assert!(position(&names, calm) < position(&names, hot), "{names:?}");
        }
    }

    let ledger = &symbol(&report, "BillingLedger")["rank"];
    let components: Vec<&str> = ledger["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|component| component["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        components,
        vec!["calm", "mature", "used", "substance", "lineage"]
    );
    assert_eq!(ledger["components"][1]["value"], 1.0, "years old: mature");
    assert_eq!(
        ledger["components"][2]["value"], 1.0,
        "called from checkout"
    );
    assert_eq!(ledger["components"][3]["factor"], true);
    // Every class of this fixture sits in a file of six lines.
    assert_eq!(ledger["components"][3]["value"], 0.5);
    assert_eq!(ledger["proven"]["stub"], "short_file");
    assert_eq!(ledger["components"][4]["value"], 1.0, "no base to judge");
}

fn worker(name: &str, base: &str) -> String {
    let steps: String = (1..=8)
        .map(|step| format!("  def step{step}(job)\n    job.advance({step})\n  end\n\n"))
        .collect();
    format!("class {name} < {base}\n{steps}end\n")
}

/// Two families of workers that differ only in whether the code base still
/// adds members to them: six `LegacyBase` workers from 2019 and six
/// `LiveBase` workers from 2025, all substantial, all called from `Runner`.
/// `LiveBase` also has a one-line stub subclass and an empty one.
fn worker_project() -> Workspace {
    let ws = workspace();
    git_in(&ws.root, &[], &["init", "-q", "-b", "main"]);
    ws.commit(
        "2019-01-01T10:00:00",
        "dev1",
        "Add legacy base",
        &[(
            "app/legacy_base.rb",
            "class LegacyBase\n  def perform\n  end\nend\n",
        )],
    );
    for i in 0..6 {
        ws.commit(
            &format!("2019-02-0{}T10:00:00", i + 1),
            "dev1",
            &format!("Add legacy worker {i}"),
            &[(
                &format!("app/legacy/legacy_worker{i}.rb"),
                &worker(&format!("LegacyWorker{i}"), "LegacyBase"),
            )],
        );
    }
    ws.commit(
        "2025-01-01T10:00:00",
        "dev2",
        "Add live base",
        &[(
            "app/live_base.rb",
            "class LiveBase\n  def perform\n  end\nend\n",
        )],
    );
    for i in 0..6 {
        ws.commit(
            &format!("2025-02-0{}T10:00:00", i + 1),
            "dev2",
            &format!("Add live worker {i}"),
            &[(
                &format!("app/live/live_worker{i}.rb"),
                &worker(&format!("LiveWorker{i}"), "LiveBase"),
            )],
        );
    }
    ws.commit(
        "2025-03-01T10:00:00",
        "dev2",
        "Add stub workers",
        &[
            (
                "app/live/stub_worker.rb",
                "class LiveStubWorker < LiveBase; end\n",
            ),
            (
                "app/live/empty_worker.rb",
                &format!(
                    "class LiveEmptyWorker < LiveBase\nend\n\n{}",
                    worker("LiveEmptyWorkerHelper", "Object")
                ),
            ),
        ],
    );
    let calls: String = (0..6)
        .map(|i| format!("    LegacyWorker{i}.new.perform\n    LiveWorker{i}.new.perform\n"))
        .collect();
    ws.commit(
        "2025-04-01T10:00:00",
        "dev3",
        "Add runner",
        &[(
            "app/runner.rb",
            &format!(
                "class Runner\n  def call\n{calls}    LiveStubWorker.new.perform\n    LiveEmptyWorker.new.perform\n  end\nend\n"
            ),
        )],
    );
    ws
}

#[test]
fn proven_prefers_a_live_lineage_and_real_code_over_stubs() {
    let ws = worker_project();
    collect_everything(&ws);

    let report = ws.json(&["search", "Worker", "--fuzzy", "--rank", "proven"]);
    let names = symbol_names(&report);
    let live: Vec<usize> = (0..6)
        .map(|i| position(&names, &format!("LiveWorker{i}")))
        .collect();
    let legacy: Vec<usize> = (0..6)
        .map(|i| position(&names, &format!("LegacyWorker{i}")))
        .collect();
    let stubs = [
        position(&names, "LiveStubWorker"),
        position(&names, "LiveEmptyWorker"),
    ];
    assert!(
        live.iter().max() < legacy.iter().min(),
        "a live family leads a dying one: {names:?}"
    );
    assert!(
        live.iter().max() < stubs.iter().min(),
        "real code leads stubs of the same family: {names:?}"
    );

    let legacy = &symbol(&report, "LegacyWorker0")["rank"];
    assert_eq!(legacy["proven"]["lineage"]["base"], "LegacyBase");
    assert_eq!(legacy["proven"]["lineage"]["subclasses"], 6);
    assert_eq!(legacy["proven"]["lineage"]["recent"], 0);
    assert_eq!(legacy["components"][4]["value"], 0.5);
    assert_eq!(
        legacy["components"][1]["value"], 1.0,
        "seven years old counts no more than half a year"
    );
    let live = &symbol(&report, "LiveWorker0")["rank"];
    assert_eq!(live["proven"]["lineage"]["base"], "LiveBase");
    assert_eq!(live["components"][4]["value"], 1.0);
    assert!(live["proven"].get("stub").is_none());
    assert_eq!(
        symbol(&report, "LiveStubWorker")["rank"]["proven"]["stub"],
        "short_file"
    );
    assert_eq!(
        symbol(&report, "LiveEmptyWorker")["rank"]["proven"]["stub"],
        "empty_class_body"
    );

    let text = ws.run(&["search", "Worker", "--fuzzy", "--rank", "proven"]);
    assert!(
        text.contains(
            "lineage: weakest base LegacyBase — 0 of 6 subclasses added in the last 2 years"
        ),
        "{text}"
    );
    assert!(text.contains("substance: stub, empty class body"), "{text}");
}

#[test]
fn risky_preset_needs_both_dependents_and_an_unstable_history() {
    let ws = billing_project();
    collect_everything(&ws);

    let report = ws.json(&["search", "Billing", "--rank", "risky"]);
    let names = symbol_names(&report);
    assert_eq!(names[1], "BillingRetry", "{names:?}");
    let draft = &symbol(&report, "BillingDraft")["rank"];
    assert_eq!(draft["graph"]["dependents"], 0);
    assert!(draft["history"]["hotspot_score"].as_u64().unwrap() >= 80);
    assert_eq!(
        draft["score"], 0.0,
        "a hotspot nothing depends on is not risky"
    );
    assert!(
        position(&names, "BillingCore") < position(&names, "BillingDraft"),
        "{names:?}"
    );
    let unused = &symbol(&report, "Billing")["rank"];
    assert_eq!(unused["score"], 0.0, "no dependents means no blast radius");
}

#[test]
fn central_preset_follows_pagerank_and_ignores_history() {
    let ws = billing_project();
    ws.run(&["rebuild"]);
    ws.run(&["graph", "build"]);

    let report = ws.json(&["search", "Billing", "--rank", "central"]);
    assert_eq!(report["rank"]["applied"], true, "central needs no history");
    assert!(report["rank"].get("history").is_none());
    let names = symbol_names(&report);
    assert_eq!(
        names,
        vec![
            "Billing",
            "BillingRetry",
            "BillingCore",
            "BillingLedger",
            "BillingDraft"
        ]
    );
    let retry = &symbol(&report, "BillingRetry")["rank"];
    assert!(retry.get("history").is_none(), "central carries no history");
    assert_eq!(retry["graph"]["fan_in_files"], 3);
    assert_eq!(symbol(&report, "BillingDraft")["rank"]["score"], 0.0);
}

#[test]
fn preset_without_evidence_is_not_applied_and_names_the_commands() {
    let ws = billing_project();
    ws.run(&["rebuild"]);

    let plain = symbol_names(&ws.json(&["search", "Billing"]));
    let report = ws.json(&["search", "Billing", "--rank", "proven"]);
    assert_eq!(report["rank"]["applied"], false);
    let missing: Vec<(&str, &str)> = report["rank"]["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["signal"].as_str().unwrap(),
                item["command"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(
        missing.contains(&("graph", "ast-index graph build")),
        "{missing:?}"
    );
    assert!(
        missing.contains(&("history", "ast-index hotspots --collect")),
        "{missing:?}"
    );
    assert_eq!(
        symbol_names(&report),
        plain,
        "unranked results keep relevance order"
    );
    assert!(report["symbols"][0]["rank"].is_null());
    assert!(report["files"][0]["rank"].is_null());

    let text = ws.run(&["search", "Billing", "--rank", "proven"]);
    assert!(text.contains("NOT applied"), "{text}");
    assert!(text.contains("ast-index hotspots --collect"), "{text}");
    assert!(text.contains("ast-index graph build"), "{text}");

    ws.run(&["hotspots", "--collect"]);
    let hotspots = ws.json(&["search", "Billing", "--rank", "hotspots"]);
    assert_eq!(
        hotspots["rank"]["applied"], true,
        "hotspots needs history only"
    );
    let risky = ws.json(&["search", "Billing", "--rank", "risky"]);
    assert_eq!(risky["rank"]["applied"], false);
    assert_eq!(risky["rank"]["missing"][0]["signal"], "graph");
}

#[test]
fn stale_graph_is_reported_but_still_ranks() {
    let ws = billing_project();
    collect_everything(&ws);
    ws.write("app/extra.rb", &ruby_class("BillingExtra", ""));
    ws.run(&["update"]);

    let report = ws.json(&["search", "Billing", "--rank", "central"]);
    assert_eq!(report["rank"]["applied"], true);
    assert_eq!(report["rank"]["graph"]["stale"], true);
    assert!(
        report["rank"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("graph build"),
        "{}",
        report["rank"]
    );
    let text = ws.run(&["search", "Billing", "--rank", "central"]);
    assert!(text.contains("stale"), "{text}");
}

#[test]
fn vendor_code_is_never_scored_and_stays_behind_project_code() {
    let ws = billing_project();
    ws.write(
        "node_modules/billing-kit/index.d.ts",
        "export declare class BillingKit {\n  charge(): void;\n}\n",
    );
    collect_everything(&ws);

    let report = ws.json(&["search", "Billing", "--rank", "proven"]);
    let names = symbol_names(&report);
    let kit = &symbol(&report, "BillingKit")["rank"];
    assert_eq!(kit["unscored"], "vendor");
    assert!(kit["score"].is_null());
    for project in [
        "BillingCore",
        "BillingRetry",
        "BillingLedger",
        "BillingDraft",
    ] {
        assert!(
            position(&names, project) < position(&names, "BillingKit"),
            "{names:?}"
        );
    }
    let text = ws.run(&["search", "Billing", "--rank", "proven"]);
    assert!(text.contains("third-party code has no history"), "{text}");
}

#[test]
fn ranked_json_carries_the_dossier_for_files_and_symbols() {
    let ws = billing_project();
    collect_everything(&ws);

    let report = ws.json(&["search", "Billing", "--rank", "risky", "-l", "3"]);
    let rank = &report["rank"];
    assert_eq!(rank["preset"], "risky");
    assert_eq!(rank["applied"], true);
    assert!(rank["formula"].as_str().unwrap().contains("blast radius"));
    assert_eq!(rank["history"]["granularity"], "file");
    assert_eq!(rank["graph"]["granularity"], "symbol");
    assert!(rank["pool"]["symbols"].as_u64().unwrap() >= 4);
    assert_eq!(report["symbols"].as_array().unwrap().len(), 3);
    assert_eq!(report["pagination"]["symbols"]["total"], 5);

    let retry = &symbol(&report, "BillingRetry")["rank"];
    for key in ["score", "blended", "relevance_rank", "tier", "components"] {
        assert!(!retry[key].is_null(), "missing {key}: {retry:#}");
    }
    assert_eq!(retry["tier"], "name");
    assert_eq!(retry["history"]["granularity"], "file");
    assert_eq!(retry["history"]["commits"], 7);
    assert_eq!(retry["history"]["fix_commits"], 6);
    assert!(
        retry["history"]["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label.as_str().unwrap().starts_with("fixes:")),
        "{retry:#}"
    );
    assert_eq!(retry["graph"]["granularity"], "symbol");
    assert_eq!(retry["graph"]["fan_in_files"], 3);

    let file = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "app/billing_retry.rb")
        .unwrap_or_else(|| panic!("billing_retry.rb in {:#}", report["files"]));
    assert_eq!(file["rank"]["graph"]["granularity"], "file");
    assert_eq!(
        file["rank"]["graph"]["strongest_symbol"]["name"],
        "BillingRetry"
    );
    assert!(file["rank"].get("relevance_rank").is_none());
    assert_eq!(
        report["files"][0]["path"], "app/billing_retry.rb",
        "a path tier weighs in but does not outrank the score: {:#}",
        report["files"]
    );
    assert_eq!(report["files"][0]["rank"]["tier"], "file_name");

    let text = ws.run(&["search", "Billing", "--rank", "risky"]);
    assert!(text.contains("file history: 7 commits"), "{text}");
    assert!(text.contains("symbol graph: fan-in"), "{text}");
    assert!(
        text.contains("graph via strongest symbol BillingRetry"),
        "{text}"
    );
}

#[test]
fn unknown_preset_is_rejected() {
    let ws = billing_project();
    ws.run(&["rebuild"]);
    let output = ws.ast_index(&["search", "Billing", "--rank", "shiny"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("proven"));
}

#[test]
fn exclude_tests_leaves_test_files_out_of_ranked_sections() {
    let ws = billing_project();
    for round in 1..=8 {
        let body = format!("    amount if {round} > 0\n");
        ws.commit(
            &format!("2026-0{}-20T10:00:00", round % 9 + 1),
            "dev1",
            &format!("Fix billing spec helper {round}"),
            &[(
                "spec/billing_retry_spec.rb",
                &ruby_class("BillingSpecHelper", &body),
            )],
        );
    }
    collect_everything(&ws);

    let spec = "spec/billing_retry_spec.rb";
    let file_paths = |report: &Value| -> Vec<String> {
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap().to_string())
            .collect()
    };
    let symbol_paths = |report: &Value| -> Vec<String> {
        report["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|symbol| symbol["path"].as_str().unwrap().to_string())
            .collect()
    };

    let with_tests = ws.json(&["search", "Billing", "--rank", "hotspots"]);
    assert!(file_paths(&with_tests).contains(&spec.to_string()));
    assert!(symbol_paths(&with_tests).contains(&spec.to_string()));

    let without = ws.json(&["search", "Billing", "--rank", "hotspots", "--exclude-tests"]);
    assert!(!file_paths(&without).contains(&spec.to_string()));
    assert!(!symbol_paths(&without).contains(&spec.to_string()));
    assert_eq!(without["rank"]["pool"]["tests_excluded"], true);
    assert_eq!(
        without["pagination"]["files"]["total"].as_u64().unwrap() + 1,
        with_tests["pagination"]["files"]["total"].as_u64().unwrap()
    );
    assert_eq!(
        without["pagination"]["symbols"]["total"].as_u64().unwrap() + 1,
        with_tests["pagination"]["symbols"]["total"]
            .as_u64()
            .unwrap()
    );
    // The file's history is ranked against every file either way.
    let retry = |report: &Value| {
        report["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|symbol| symbol["name"] == "BillingRetry")
            .unwrap()["rank"]["score"]
            .clone()
    };
    assert_eq!(retry(&with_tests), retry(&without));

    let text = ws.run(&["search", "Billing", "--rank", "hotspots", "--exclude-tests"]);
    assert!(text.contains("test files left out"), "{text}");

    let plain = ws.ast_index(&["search", "Billing", "--exclude-tests"]);
    assert!(!plain.status.success(), "--exclude-tests needs --rank");
    assert!(String::from_utf8_lossy(&plain.stderr).contains("--rank"));
}
