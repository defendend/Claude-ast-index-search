//! Name lookups for symbols whose index name is qualified.
//!
//! Ruby records `class Billing::LedgerImporter` under its full name and
//! leaves `qualified_name` empty (only C++ fills it). A lookup by the short
//! name has no exact row, and one by the full name used to read
//! `qualified_name` only; both have to find the class.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use ast_index::db::{self, SearchScope, SymbolKind};
use tempfile::TempDir;

fn run(root: &Path, cache: &Path, args: &[&str]) -> String {
    let output: Output = Command::new(env!("CARGO_BIN_EXE_ast-index"))
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
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A namespaced importer with a base class, a subclass, a caller that names
/// it in full and an unrelated `LedgerImporter` reference in another
/// namespace.
fn ruby_project() -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let files = [
        (
            "app/services/billing/base_importer.rb",
            "module Billing\n  class BaseImporter\n  end\nend\n",
        ),
        (
            "app/services/billing/ledger_importer.rb",
            "class Billing::LedgerImporter < Billing::BaseImporter\n  def self.build\n    new\n  end\nend\n",
        ),
        (
            "app/services/billing/csv_importer.rb",
            "class Billing::CsvImporter < Billing::LedgerImporter\nend\n",
        ),
        (
            "app/jobs/import_job.rb",
            "class ImportJob\n  def perform\n    Billing::LedgerImporter.build\n  end\nend\n",
        ),
        (
            "lib/archive/reader.rb",
            "class Archive::Reader\n  def read\n    LedgerImporter.new\n  end\nend\n",
        ),
    ];
    for (path, content) in files {
        let path = project.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    run(project.path(), cache.path(), &["rebuild"]);
    (project, cache)
}

#[test]
fn short_and_full_names_find_a_namespaced_ruby_class() {
    let (project, cache) = ruby_project();
    let run = |args: &[&str]| run(project.path(), cache.path(), args);
    let definition = "Billing::LedgerImporter [class]: app/services/billing/ledger_importer.rb:1";

    for name in ["LedgerImporter", "Billing::LedgerImporter"] {
        let hierarchy = run(&["hierarchy", name]);
        assert!(
            hierarchy.starts_with("Hierarchy for 'Billing::LedgerImporter':"),
            "{hierarchy}"
        );
        assert!(
            hierarchy.contains("Billing::BaseImporter (extends)"),
            "{hierarchy}"
        );
        assert!(
            hierarchy.contains("Billing::CsvImporter [class]"),
            "{hierarchy}"
        );

        for command in ["class", "symbol"] {
            let found = run(&[command, name]);
            assert!(
                found.contains("(showing 1 of 1)"),
                "{command} {name}: {found}"
            );
            assert!(found.contains(definition), "{command} {name}: {found}");
        }

        let refs = run(&["refs", name]);
        assert!(refs.contains("Definitions (showing 1 of 1)"), "{refs}");
        assert!(refs.contains(definition), "{refs}");

        let implementations = run(&["implementations", name]);
        assert!(
            implementations.contains("Billing::CsvImporter [class]"),
            "{implementations}"
        );
    }
}

#[test]
fn usages_of_a_full_name_read_the_lines_that_spell_it_out() {
    let (project, cache) = ruby_project();
    let run = |args: &[&str]| run(project.path(), cache.path(), args);

    let usages = run(&["usages", "Billing::LedgerImporter"]);
    assert!(usages.contains("app/jobs/import_job.rb:3"), "{usages}");
    assert!(usages.contains("recorded as 'LedgerImporter'"), "{usages}");
    assert!(!usages.contains("lib/archive/reader.rb"), "{usages}");

    let refs = run(&["refs", "Billing::LedgerImporter"]);
    assert!(refs.contains("app/jobs/import_job.rb:3"), "{refs}");
    assert!(!refs.contains("lib/archive/reader.rb"), "{refs}");

    // The short name keeps every reference recorded under it.
    let short = run(&["usages", "LedgerImporter"]);
    assert!(short.contains("lib/archive/reader.rb:3"), "{short}");
    assert!(!short.contains("recorded as"), "{short}");
}

#[test]
fn unused_symbols_look_up_references_by_the_last_segment() {
    let (project, cache) = ruby_project();
    let unused = run(project.path(), cache.path(), &["unused-symbols"]);
    assert!(!unused.contains("Billing::LedgerImporter "), "{unused}");
    assert!(!unused.contains("self.build"), "{unused}");
    assert!(unused.contains("perform [function]"), "{unused}");
}

fn open_fresh_db(project_root: &Path) -> rusqlite::Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn names(results: &[db::SearchResult]) -> Vec<&str> {
    results.iter().map(|r| r.display_name()).collect()
}

#[test]
fn qualified_names_of_both_kinds_resolve_and_an_exact_name_still_wins() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    // C++ keeps the bare name in `name` and the full one in `qualified_name`.
    let header = db::upsert_file(&conn, "src/ledger.h", 0, 100).unwrap();
    conn.execute(
        "INSERT INTO symbols (file_id, name, qualified_name, kind, line) \
         VALUES (?1, 'Ledger', 'books::Ledger', 'class', 3)",
        [header],
    )
    .unwrap();
    let ruby = db::upsert_file(&conn, "app/models/billing/invoice.rb", 0, 100).unwrap();
    db::insert_symbol(&conn, ruby, "Billing::Invoice", SymbolKind::Class, 1, None).unwrap();
    let import = db::upsert_file(&conn, "src/main.rs", 0, 100).unwrap();
    db::insert_symbol(&conn, import, "crate::Invoice", SymbolKind::Import, 1, None).unwrap();

    let none = SearchScope::none();
    let scoped = SearchScope {
        in_file: None,
        module: Some(""),
        dir_prefix: None,
    };
    for name in ["Invoice", "Billing::Invoice", "::Invoice"] {
        for scope in [&none, &scoped] {
            if name != "::Invoice" {
                // `::Invoice` asks for any namespaced `Invoice`, the import
                // of `crate::Invoice` included.
                assert_eq!(
                    names(&db::find_symbols_by_name_scoped(&conn, name, None, 10, scope).unwrap()),
                    ["Billing::Invoice"],
                    "{name}"
                );
                assert_eq!(
                    db::count_symbols_by_name_scoped(&conn, name, None, scope, false).unwrap(),
                    1,
                    "{name}"
                );
            }
            assert_eq!(
                names(&db::find_class_like_scoped(&conn, name, 10, scope).unwrap()),
                ["Billing::Invoice"],
                "{name}"
            );
            assert_eq!(
                db::count_class_like_scoped(&conn, name, scope).unwrap(),
                1,
                "{name}"
            );
            assert_eq!(
                names(&db::find_definitions_scoped(&conn, name, 10, scope).unwrap()),
                ["Billing::Invoice"],
                "{name}"
            );
        }
    }
    for name in ["Ledger", "books::Ledger", "::Ledger"] {
        assert_eq!(
            names(&db::find_symbols_by_name(&conn, name, Some("class"), 10).unwrap()),
            ["books::Ledger"],
            "{name}"
        );
    }

    // A top-level class of the same short name is an exact hit and wins.
    let top = db::upsert_file(&conn, "app/models/invoice.rb", 0, 100).unwrap();
    db::insert_symbol(&conn, top, "Invoice", SymbolKind::Class, 1, None).unwrap();
    assert_eq!(
        names(&db::find_symbols_by_name(&conn, "Invoice", None, 10).unwrap()),
        ["Invoice"]
    );
    assert_eq!(
        names(&db::find_symbols_by_name(&conn, "Billing::Invoice", None, 10).unwrap()),
        ["Billing::Invoice"]
    );
}
