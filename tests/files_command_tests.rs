//! Integration tests for `commands::files` public APIs.
//!
//! Covers the file-oriented commands (`cmd_file`, `cmd_outline`,
//! `cmd_imports`) that are user-facing entry points.

use std::{fs, process::Command};

use ast_index::commands::files::{cmd_file, cmd_imports, cmd_outline};
use ast_index::db;
use tempfile::TempDir;

fn open_fresh_db(project_root: &std::path::Path) -> rusqlite::Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

// ----------------------------------------------------------------------
// cmd_file
// ----------------------------------------------------------------------

#[test]
fn cmd_file_returns_ok_without_index() {
    let dir = TempDir::new().unwrap();
    // No DB present — must short-circuit with an Ok message, not error.
    cmd_file(dir.path(), "anything", false, 10, "text")
        .expect("cmd_file must not error without index");
}

#[test]
fn cmd_file_finds_indexed_files() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    db::upsert_file(&conn, "src/main/Foo.kt", 0, 100).unwrap();
    db::upsert_file(&conn, "src/main/Bar.kt", 0, 100).unwrap();
    drop(conn);

    cmd_file(dir.path(), "Foo", false, 10, "text").unwrap();

    // Verify via the same public DB API the command uses internally.
    let conn = db::open_db(dir.path()).unwrap();
    let hits = db::find_files(&conn, "Foo", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].ends_with("Foo.kt"));
}

#[test]
fn file_cli_emits_json_when_requested() {
    let dir = TempDir::new().unwrap();
    let conn = open_fresh_db(dir.path());
    db::upsert_file(&conn, "src/main/Foo.kt", 0, 100).unwrap();
    drop(conn);

    let out = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(dir.path())
        .args(["file", "Foo", "--format", "json"])
        .output()
        .expect("ast-index binary must run");

    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let files: Vec<String> = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be valid JSON: {e}; stdout={stdout}"));
    assert_eq!(files.len(), 1, "expected exactly one file; stdout={stdout}");
    assert!(
        files[0].ends_with("src/main/Foo.kt"),
        "resolved file path must end with indexed file path; stdout={stdout}"
    );
    assert!(
        !out.stdout.windows(2).any(|w| w == b"\x1b["),
        "JSON output must not contain ANSI codes; stdout={stdout}"
    );
}

// ----------------------------------------------------------------------
// cmd_outline
// ----------------------------------------------------------------------

#[test]
fn cmd_outline_handles_missing_file_gracefully() {
    let dir = TempDir::new().unwrap();
    cmd_outline(dir.path(), "does/not/exist.kt", "text")
        .expect("missing file must print a hint, not error");
}

#[test]
fn cmd_outline_parses_a_kotlin_file() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("Foo.kt");
    fs::write(&src, "package demo\n\nclass Foo {\n  fun bar() {}\n}\n").unwrap();

    cmd_outline(dir.path(), "Foo.kt", "text").expect("outline of valid Kotlin must succeed");
}

#[test]
fn outline_names_an_anonymous_default_export_after_its_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();
    let src = dir.path().join("hooks/useMap.js");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::write(
        &src,
        "const searchPath = () => '';\n\nexport default ({ form }) => {\n  return searchPath(form);\n};\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .args(["outline", "hooks/useMap.js"])
        .output()
        .expect("ast-index binary must run");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout={stdout}");
    assert!(stdout.contains(":3-5 useMap [function]"), "stdout={stdout}");
    assert!(!stdout.contains(" default "), "stdout={stdout}");
}

#[test]
fn cmd_outline_handles_unsupported_extension() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("notes.unknown_ext_xyz");
    fs::write(&src, "hello\n").unwrap();

    cmd_outline(dir.path(), "notes.unknown_ext_xyz", "text")
        .expect("unknown extension must print a hint, not error");
}

fn run_outline(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .arg("outline")
        .args(args)
        .output()
        .expect("ast-index binary must run");
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn outline_prints_the_line_range_of_multi_line_definitions() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("billing.rb"),
        "class Invoice\n  def total\n    1\n  end\n\n  def paid?; true; end\nend\n",
    )
    .unwrap();

    let stdout = run_outline(dir.path(), &["billing.rb"]);
    assert!(stdout.contains("  :1-7 Invoice [class]"), "{stdout}");
    assert!(stdout.contains("  :2-4 total [function]"), "{stdout}");
    assert!(stdout.contains("  :6 paid? [function]"), "{stdout}");
}

#[test]
fn outline_json_lists_symbols_with_end_lines() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("billing.rb"),
        "require 'json'\n\nclass Invoice\n  def total\n    1\n  end\nend\n",
    )
    .unwrap();

    let stdout = run_outline(dir.path(), &["billing.rb", "--format", "json"]);
    assert!(
        !stdout.contains('\u{1b}'),
        "JSON must be uncoloured: {stdout}"
    );
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["file"], "billing.rb");
    assert!(doc.get("skipped").is_none(), "{doc:#}");
    let rows = doc["symbols"].as_array().unwrap();
    // Imports stay out of the outline, as in the text form.
    assert_eq!(rows.len(), 2, "{doc:#}");
    assert_eq!(rows[0]["name"], "Invoice");
    assert_eq!(rows[0]["kind"], "class");
    assert_eq!(rows[0]["line"], 3);
    assert_eq!(rows[0]["end_line"], 7);
    assert_eq!(rows[1]["name"], "total");
    assert_eq!(rows[1]["end_line"], 6);
}

#[test]
fn outline_json_says_why_it_lists_nothing() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("notes.unknown_ext_xyz"), "hello\n").unwrap();
    for (file, reason) in [
        ("missing.rb", "not_found"),
        ("notes.unknown_ext_xyz", "unsupported"),
    ] {
        let stdout = run_outline(dir.path(), &[file, "--format", "json"]);
        let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(doc["skipped"], reason, "{doc:#}");
        assert_eq!(doc["symbols"].as_array().unwrap().len(), 0);
    }
}

// ----------------------------------------------------------------------
// cmd_imports
// ----------------------------------------------------------------------

#[test]
fn cmd_imports_extracts_python_imports() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("script.py");
    fs::write(&src, "import os\nfrom typing import List\nimport sys\n").unwrap();

    cmd_imports(dir.path(), "script.py").expect("python imports must parse");
}

#[test]
fn cmd_imports_extracts_kotlin_imports() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("Foo.kt");
    fs::write(
        &src,
        "package demo\n\nimport kotlinx.coroutines.flow.Flow\nimport java.util.UUID\n\nclass Foo\n",
    )
    .unwrap();

    cmd_imports(dir.path(), "Foo.kt").expect("kotlin imports must parse");
}

#[test]
fn cmd_imports_handles_missing_file() {
    let dir = TempDir::new().unwrap();
    cmd_imports(dir.path(), "absent.kt").expect("missing file must print a hint, not error");
}

#[test]
fn cmd_imports_handles_file_with_no_imports() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("Bare.kt");
    fs::write(&src, "package demo\n\nclass Bare\n").unwrap();

    cmd_imports(dir.path(), "Bare.kt").expect("file with no imports must succeed");
}

#[test]
fn imports_lists_every_typescript_import_on_one_line() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src/Invoice.tsx");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::write(
        &src,
        concat!(
            "import React from 'react';\n",
            "import debounce from 'lodash/debounce';\n",
            "import {\n",
            "  Button,\n",
            "  Icon,\n",
            "} from 'shared/components';\n",
            "import { total } from './total';\n",
            "export * from './types';\n",
            "\n",
            "export default () => <Button>{total()}</Button>;\n",
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ast-index"))
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .args(["imports", "src/Invoice.tsx"])
        .output()
        .expect("ast-index binary must run");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout={stdout}");
    assert_eq!(
        stdout,
        concat!(
            "Imports in src/Invoice.tsx:\n",
            "  React from 'react';\n",
            "  debounce from 'lodash/debounce';\n",
            "  { Button, Icon } from 'shared/components';\n",
            "  { total } from './total';\n",
            "  export * from './types';\n",
            "\n",
            "  Total: 5 imports\n",
        )
    );
}
