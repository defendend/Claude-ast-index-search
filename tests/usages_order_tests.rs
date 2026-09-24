//! `usages` and `refs` list references in production files first, test files
//! after them, each group by path and line.
//!
//! Without that the page held whichever references had the lowest file ids,
//! specs mixed in with the code they test.

use std::fs;
use std::path::Path;
use std::process::Command;

use ast_index::db::{self, SearchScope};
use tempfile::TempDir;

fn open_fresh_db(project_root: &Path) -> rusqlite::Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn insert_refs(conn: &rusqlite::Connection, path: &str, refs: &[(&str, i64)]) {
    let file = db::upsert_file(conn, path, 0, 100).unwrap();
    for (name, line) in refs {
        conn.execute(
            "INSERT INTO refs (file_id, name, line, context) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![file, name, line, format!("{name}({line})")],
        )
        .unwrap();
    }
}

fn located(refs: &[db::RefResult]) -> Vec<String> {
    refs.iter()
        .map(|r| format!("{}:{}{}", r.path, r.line, if r.test { " test" } else { "" }))
        .collect()
}

/// Test files are indexed first, so they hold the lowest file ids, and two
/// of them sort before every production path.
fn seed(conn: &rusqlite::Connection) {
    insert_refs(
        conn,
        "engines/billing/spec/charge_spec.rb",
        &[("charge", 4), ("charge", 9)],
    );
    insert_refs(conn, "src/__tests__/charge.test.js", &[("charge", 2)]);
    insert_refs(conn, "spec/charge_spec.rb", &[("charge", 7)]);
    insert_refs(conn, "scripts/run.rb", &[("charge", 12), ("charge", 3)]);
    insert_refs(conn, "lib/billing.rb", &[("charge", 30), ("refund", 31)]);
}

#[test]
fn production_references_come_first_grouped_by_file() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    seed(&conn);

    let all = [
        "lib/billing.rb:30",
        "scripts/run.rb:3",
        "scripts/run.rb:12",
        "engines/billing/spec/charge_spec.rb:4 test",
        "engines/billing/spec/charge_spec.rb:9 test",
        "spec/charge_spec.rb:7 test",
        "src/__tests__/charge.test.js:2 test",
    ];
    let scoped = SearchScope {
        in_file: None,
        module: Some(""),
        dir_prefix: None,
    };
    assert_eq!(
        located(&db::find_references(&conn, "charge", 10).unwrap()),
        all
    );
    assert_eq!(
        located(&db::find_references_scoped(&conn, "charge", 10, &scoped).unwrap()),
        all
    );
    // A short page is a prefix of the full order, not the lowest file ids.
    assert_eq!(
        located(&db::find_references(&conn, "charge", 3).unwrap()),
        all[..3]
    );
    let lib = SearchScope {
        in_file: None,
        module: Some("lib/"),
        dir_prefix: None,
    };
    assert_eq!(
        located(&db::find_references_scoped(&conn, "charge", 10, &lib).unwrap()),
        ["lib/billing.rb:30"]
    );
    assert_eq!(
        located(
            &db::find_references_mentioning_scoped(&conn, "charge", "charge(", 2, &scoped).unwrap()
        ),
        all[..2]
    );
}

#[test]
fn usages_json_marks_test_references() {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    for (path, content) in [
        (
            "spec/billing_spec.rb",
            "describe Billing do\n  it { charge_card(1) }\nend\n",
        ),
        (
            "lib/billing.rb",
            "class Billing\n  def run\n    charge_card(2)\n  end\nend\n",
        ),
    ] {
        let path = project.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
            .current_dir(project.path())
            .env("AST_INDEX_CACHE_DIR", cache.path())
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
    };
    run(&["rebuild"]);

    let report: serde_json::Value =
        serde_json::from_str(&run(&["--format", "json", "usages", "charge_card"])).unwrap();
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{report:#}");
    assert_eq!(items[0]["path"], "lib/billing.rb");
    assert!(items[0].get("test").is_none(), "false is not serialized");
    assert_eq!(items[1]["path"], "spec/billing_spec.rb");
    assert_eq!(items[1]["test"], true);

    let text = run(&["usages", "charge_card"]);
    let lib = text.find("lib/billing.rb:3").unwrap();
    let spec = text.find("spec/billing_spec.rb:2").unwrap();
    assert!(lib < spec, "{text}");
}
