//! Coverage for `symbols.end_line` — the last line of a definition.
//!
//! Only parsers that report a range fill it; everything else stores NULL,
//! so these tests pin both the filled and the unfilled side.

use std::fs;
use std::path::Path;

use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    conn
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// (name, line, end_line) for every symbol of the given kind.
fn symbol_ranges(conn: &Connection, kind: &str) -> Vec<(String, i64, Option<i64>)> {
    let mut stmt = conn
        .prepare("SELECT name, line, end_line FROM symbols WHERE kind = ?1 ORDER BY line")
        .unwrap();
    stmt.query_map([kind], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn range_of(ranges: &[(String, i64, Option<i64>)], name: &str) -> (i64, i64) {
    let hit = ranges
        .iter()
        .find(|(n, _, _)| n == name)
        .unwrap_or_else(|| panic!("symbol {name} not indexed, got {ranges:?}"));
    (
        hit.1,
        hit.2
            .unwrap_or_else(|| panic!("symbol {name} has no end_line")),
    )
}

/// Index `contents` as the only file of a fresh project.
fn index_single(relative_path: &str, contents: &str) -> Connection {
    let project = TempDir::new().unwrap();
    write_file(&project.path().join(relative_path), contents);
    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();
    conn
}

/// Start and end line of the symbol with this kind and name.
fn span(conn: &Connection, kind: &str, name: &str) -> (i64, i64) {
    range_of(&symbol_ranges(conn, kind), name)
}

fn assert_encloses(outer: (i64, i64), inner: (i64, i64)) {
    assert!(
        outer.0 <= inner.0 && inner.1 <= outer.1 && outer != inner,
        "{inner:?} must nest strictly inside {outer:?}"
    );
}

/// Every symbol of the file reports a range that does not end before it starts.
fn assert_all_ranges_filled(conn: &Connection) {
    let mut stmt = conn
        .prepare("SELECT kind, name, line, end_line FROM symbols ORDER BY line")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!rows.is_empty(), "the file must yield symbols");
    for (kind, name, line, end_line) in rows {
        let end_line = end_line.unwrap_or_else(|| panic!("{kind} {name} has no end_line"));
        assert!(
            end_line >= line,
            "{kind} {name} ends at {end_line} before {line}"
        );
    }
}

#[test]
fn ruby_class_range_encloses_its_methods() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("app/greeter.rb"),
        "class Greeter\n  def hello\n    'hi'\n  end\n\n  def bye\n    'bye'\n  end\nend\n",
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    let classes = symbol_ranges(&conn, "class");
    let functions = symbol_ranges(&conn, "function");

    let (class_start, class_end) = range_of(&classes, "Greeter");
    assert_eq!((class_start, class_end), (1, 9));

    let (hello_start, hello_end) = range_of(&functions, "hello");
    assert_eq!((hello_start, hello_end), (2, 4));

    let (bye_start, bye_end) = range_of(&functions, "bye");
    assert_eq!((bye_start, bye_end), (6, 8));

    for (start, end) in [(hello_start, hello_end), (bye_start, bye_end)] {
        assert!(
            class_start <= start && end <= class_end,
            "method {start}..{end} must nest inside class {class_start}..{class_end}"
        );
    }
}

#[test]
fn ruby_singleton_method_and_constant_get_ranges() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("app/config.rb"),
        "LIMITS = {\n  max: 1,\n}.freeze\n\ndef self.build\n  LIMITS\nend\n",
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    let constants = symbol_ranges(&conn, "constant");
    assert_eq!(range_of(&constants, "LIMITS"), (1, 3));

    let functions = symbol_ranges(&conn, "function");
    assert_eq!(range_of(&functions, "self.build"), (5, 7));
}

#[test]
fn typescript_class_method_and_function_get_ranges() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("src/widget.ts"),
        concat!(
            "export class Widget {\n",
            "  render(): string {\n",
            "    return 'w';\n",
            "  }\n",
            "}\n",
            "\n",
            "export function build(): Widget {\n",
            "  return new Widget();\n",
            "}\n",
        ),
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    let classes = symbol_ranges(&conn, "class");
    let functions = symbol_ranges(&conn, "function");

    let (class_start, class_end) = range_of(&classes, "Widget");
    assert_eq!((class_start, class_end), (1, 5));

    let (render_start, render_end) = range_of(&functions, "render");
    assert_eq!((render_start, render_end), (2, 4));
    assert!(class_start <= render_start && render_end <= class_end);

    assert_eq!(range_of(&functions, "build"), (7, 9));
}

#[test]
fn typescript_interface_and_arrow_function_get_ranges() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("src/shapes.ts"),
        concat!(
            "export interface Shape {\n",
            "  area: number;\n",
            "}\n",
            "\n",
            "export const draw = (shape: Shape) => {\n",
            "  return shape.area;\n",
            "};\n",
        ),
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    assert_eq!(
        range_of(&symbol_ranges(&conn, "interface"), "Shape"),
        (1, 3)
    );
    assert_eq!(range_of(&symbol_ranges(&conn, "function"), "draw"), (5, 7));
}

#[test]
fn every_end_line_is_at_or_after_its_start_line() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("app/models/user.rb"),
        concat!(
            "module Accounts\n",
            "  class User < Base\n",
            "    include Comparable\n",
            "    has_many :posts\n",
            "\n",
            "    def name\n",
            "      @name\n",
            "    end\n",
            "  end\n",
            "end\n",
        ),
    );
    write_file(
        &project.path().join("src/index.ts"),
        "export const VALUES = [\n  1,\n  2,\n];\n\nexport type Id = string;\n",
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    let broken: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE end_line IS NOT NULL AND end_line < line",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(broken, 0);

    let filled: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE end_line IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(filled > 0, "ruby and typescript must report ranges");
}

#[test]
fn parsers_without_range_support_store_null() {
    let project = TempDir::new().unwrap();
    write_file(
        &project.path().join("main.go"),
        "package main\n\nfunc Run() int {\n\treturn 1\n}\n",
    );

    let mut conn = fresh_db();
    indexer::index_directory(&mut conn, project.path(), false, false).unwrap();

    let ranges = symbol_ranges(&conn, "function");
    let run = ranges
        .iter()
        .find(|(n, _, _)| n == "Run")
        .expect("go function must be indexed");
    assert_eq!(run.2, None, "go parser does not report ranges yet");
}

#[test]
fn python_class_methods_and_function_get_ranges() {
    let conn = index_single(
        "app/greeter.py",
        concat!(
            "import os\n",
            "\n",
            "class Greeter(Base):\n",
            "    def hello(self):\n",
            "        return greet()\n",
            "\n",
            "    @property\n",
            "    def name(self):\n",
            "        return \"g\"\n",
            "\n",
            "def build():\n",
            "    return Greeter()\n",
        ),
    );

    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (3, 9));
    assert_eq!(span(&conn, "function", "hello"), (4, 5));
    // The decorator sits outside the method: it runs in the class body.
    assert_eq!(span(&conn, "function", "name"), (8, 9));
    assert_eq!(span(&conn, "annotation", "@property"), (7, 7));
    assert_encloses(class, span(&conn, "function", "hello"));
    assert_encloses(class, span(&conn, "function", "name"));
    assert_eq!(span(&conn, "function", "build"), (11, 12));
    assert_all_ranges_filled(&conn);
}
