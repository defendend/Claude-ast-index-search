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
    // Perl is parsed by regular expressions, with no syntax tree to take a
    // range from.
    let conn = index_single(
        "lib/Run.pm",
        "package main;\n\nsub run {\n  return 1;\n}\n1;\n",
    );

    let ranges = symbol_ranges(&conn, "function");
    let run = ranges
        .iter()
        .find(|(n, _, _)| n == "run")
        .expect("perl sub must be indexed");
    assert_eq!(run.2, None, "perl parser does not report ranges");
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

#[test]
fn go_struct_methods_and_function_get_ranges() {
    let conn = index_single(
        "server.go",
        concat!(
            "package main\n",
            "\n",
            "import \"fmt\"\n",
            "\n",
            "type Server struct {\n",
            "\taddr string\n",
            "}\n",
            "\n",
            "func (s *Server) Start() error {\n",
            "\treturn run(s.addr)\n",
            "}\n",
            "\n",
            "func (s Server) Stop() {\n",
            "\tfmt.Println(\"stop\")\n",
            "}\n",
            "\n",
            "func run(addr string) error {\n",
            "\treturn nil\n",
            "}\n",
        ),
    );

    // Go declares methods beside the struct, not inside it.
    assert_eq!(span(&conn, "class", "Server"), (5, 7));
    assert_eq!(span(&conn, "function", "Start"), (9, 11));
    assert_eq!(span(&conn, "function", "Stop"), (13, 15));
    assert_eq!(span(&conn, "function", "run"), (17, 19));
    assert_eq!(span(&conn, "import", "fmt"), (3, 3));
    assert_all_ranges_filled(&conn);
}

#[test]
fn rust_impl_block_encloses_its_methods() {
    let conn = index_single(
        "src/point.rs",
        concat!(
            "pub struct Point {\n",
            "    x: i32,\n",
            "}\n",
            "\n",
            "impl Point {\n",
            "    pub fn new(x: i32) -> Self {\n",
            "        Point { x }\n",
            "    }\n",
            "\n",
            "    #[inline]\n",
            "    fn get(&self) -> i32 {\n",
            "        self.x\n",
            "    }\n",
            "}\n",
            "\n",
            "pub fn run() {\n",
            "    let p = Point::new(1);\n",
            "}\n",
            "\n",
            "mod tests {\n",
            "    fn works() {\n",
            "        super::run();\n",
            "    }\n",
            "}\n",
        ),
    );

    assert_eq!(span(&conn, "class", "Point"), (1, 3));
    let block = span(&conn, "class", "impl Point");
    assert_eq!(block, (5, 14));
    assert_eq!(span(&conn, "function", "new"), (6, 8));
    // The attribute belongs to the impl block around the method.
    assert_eq!(span(&conn, "function", "get"), (11, 13));
    assert_eq!(span(&conn, "annotation", "#[inline]"), (10, 10));
    assert_encloses(block, span(&conn, "function", "new"));
    assert_encloses(block, span(&conn, "function", "get"));
    assert_eq!(span(&conn, "function", "run"), (16, 18));
    let module = span(&conn, "package", "tests");
    assert_eq!(module, (20, 24));
    assert_encloses(module, span(&conn, "function", "works"));
    assert_all_ranges_filled(&conn);
}

#[test]
fn java_class_encloses_methods_and_nested_class() {
    let conn = index_single(
        "src/main/java/x/Greeter.java",
        concat!(
            "package x;\n",
            "\n",
            "@Service\n",
            "public class Greeter {\n",
            "    private int count = 0;\n",
            "\n",
            "    @Override\n",
            "    public String hello() {\n",
            "        return greet();\n",
            "    }\n",
            "\n",
            "    public void bye() {\n",
            "        run();\n",
            "    }\n",
            "\n",
            "    static class Inner {\n",
            "        void deep() {}\n",
            "    }\n",
            "}\n",
        ),
    );

    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (4, 19));
    assert_eq!(span(&conn, "function", "hello"), (8, 10));
    assert_eq!(span(&conn, "annotation", "@Override"), (7, 7));
    assert_eq!(span(&conn, "function", "bye"), (12, 14));
    assert_eq!(span(&conn, "property", "count"), (5, 5));
    let inner = span(&conn, "class", "Inner");
    assert_eq!(inner, (16, 18));
    for member in [
        span(&conn, "function", "hello"),
        span(&conn, "function", "bye"),
        inner,
    ] {
        assert_encloses(class, member);
    }
    assert_encloses(inner, span(&conn, "function", "deep"));
    assert_all_ranges_filled(&conn);
}

#[test]
fn kotlin_class_methods_and_function_get_ranges() {
    let conn = index_single(
        "src/main/kotlin/x/Greeter.kt",
        concat!(
            "package x\n",
            "\n",
            "class Greeter(private val repo: Repo) {\n",
            "    val name: String\n",
            "        get() = compute()\n",
            "\n",
            "    fun hello(): String {\n",
            "        return greet()\n",
            "    }\n",
            "\n",
            "    fun bye() = run()\n",
            "}\n",
            "\n",
            "fun top() {\n",
            "    fun local() = 1\n",
            "}\n",
        ),
    );

    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (3, 12));
    assert_eq!(span(&conn, "property", "name"), (4, 5));
    assert_eq!(span(&conn, "function", "hello"), (7, 9));
    assert_eq!(span(&conn, "function", "bye"), (11, 11));
    assert_encloses(class, span(&conn, "function", "hello"));
    assert_encloses(class, span(&conn, "function", "bye"));
    let top = span(&conn, "function", "top");
    assert_eq!(top, (14, 16));
    assert_encloses(top, span(&conn, "function", "local"));
    assert_all_ranges_filled(&conn);
}

#[test]
fn swift_class_extension_and_function_get_ranges() {
    let conn = index_single(
        "Sources/App/Greeter.swift",
        concat!(
            "import Foundation\n",
            "\n",
            "class Greeter: Base {\n",
            "    var count = 0\n",
            "\n",
            "    init(x: Int) {\n",
            "        self.count = x\n",
            "    }\n",
            "\n",
            "    func hello() -> String {\n",
            "        return greet()\n",
            "    }\n",
            "}\n",
            "\n",
            "extension Greeter {\n",
            "    func bye() {\n",
            "        run()\n",
            "    }\n",
            "}\n",
            "\n",
            "func top() {\n",
            "    print(1)\n",
            "}\n",
        ),
    );

    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (3, 13));
    assert_eq!(span(&conn, "function", "init"), (6, 8));
    assert_eq!(span(&conn, "function", "hello"), (10, 12));
    assert_encloses(class, span(&conn, "function", "init"));
    assert_encloses(class, span(&conn, "function", "hello"));
    let extension = span(&conn, "object", "Greeter+Extension");
    assert_eq!(extension, (15, 19));
    assert_encloses(extension, span(&conn, "function", "bye"));
    assert_eq!(span(&conn, "function", "top"), (21, 23));
    assert_all_ranges_filled(&conn);
}

#[test]
fn csharp_namespace_encloses_class_and_methods() {
    let conn = index_single(
        "src/Greeter.cs",
        concat!(
            "using System;\n",
            "\n",
            "namespace App.Services\n",
            "{\n",
            "    public class Greeter\n",
            "    {\n",
            "        public string Name { get; set; }\n",
            "\n",
            "        public string Hello()\n",
            "        {\n",
            "            return Greet();\n",
            "        }\n",
            "\n",
            "        public void Bye()\n",
            "        {\n",
            "            Run();\n",
            "        }\n",
            "    }\n",
            "}\n",
        ),
    );

    let namespace = span(&conn, "package", "App.Services");
    assert_eq!(namespace, (3, 19));
    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (5, 18));
    assert_encloses(namespace, class);
    assert_eq!(span(&conn, "function", "Hello"), (9, 12));
    assert_eq!(span(&conn, "function", "Bye"), (14, 17));
    assert_encloses(class, span(&conn, "function", "Hello"));
    assert_encloses(class, span(&conn, "function", "Bye"));
    assert_all_ranges_filled(&conn);
}

#[test]
fn csharp_file_scoped_namespace_spans_the_rest_of_the_file() {
    let conn = index_single(
        "src/Foo.cs",
        concat!(
            "namespace App.Other;\n",
            "\n",
            "public class Foo\n",
            "{\n",
            "    public void Bar()\n",
            "    {\n",
            "    }\n",
            "}\n",
        ),
    );

    let namespace = span(&conn, "package", "App.Other");
    assert_eq!(namespace, (1, 8));
    assert_encloses(namespace, span(&conn, "class", "Foo"));
    assert_eq!(span(&conn, "function", "Bar"), (5, 7));
}

#[test]
fn cpp_namespace_encloses_class_and_functions() {
    let conn = index_single(
        "src/greeter.cpp",
        concat!(
            "#include <vector>\n",
            "\n",
            "#define TWICE(x) \\\n",
            "    ((x) * 2)\n",
            "\n",
            "namespace app {\n",
            "\n",
            "class Greeter : public Base {\n",
            "public:\n",
            "    int count;\n",
            "};\n",
            "\n",
            "void Greeter::hello() {\n",
            "    greet();\n",
            "}\n",
            "\n",
            "void Greeter::bye() {\n",
            "    run();\n",
            "}\n",
            "\n",
            "static int helper(int a) {\n",
            "    return a + 1;\n",
            "}\n",
            "\n",
            "}  // namespace app\n",
        ),
    );

    assert_eq!(span(&conn, "import", "vector"), (1, 1));
    // A directive owns its newline; the range stops at the last text line.
    assert_eq!(span(&conn, "constant", "TWICE"), (3, 4));
    let namespace = span(&conn, "package", "app");
    assert_eq!(namespace, (6, 25));
    assert_eq!(span(&conn, "class", "Greeter"), (8, 11));
    // Out-of-line member definitions sit beside the class, not inside it.
    assert_eq!(span(&conn, "function", "hello"), (13, 15));
    assert_eq!(span(&conn, "function", "bye"), (17, 19));
    assert_eq!(span(&conn, "function", "helper"), (21, 23));
    for symbol in [
        span(&conn, "class", "Greeter"),
        span(&conn, "function", "hello"),
        span(&conn, "function", "bye"),
        span(&conn, "function", "helper"),
    ] {
        assert_encloses(namespace, symbol);
    }
    assert_all_ranges_filled(&conn);
}

#[test]
fn php_namespace_class_methods_and_function_get_ranges() {
    let conn = index_single(
        "src/Models/User.php",
        concat!(
            "<?php\n",
            "namespace App\\Models;\n",
            "\n",
            "use Illuminate\\Support\\Str;\n",
            "\n",
            "class User extends Model\n",
            "{\n",
            "    public function greet()\n",
            "    {\n",
            "        return greet();\n",
            "    }\n",
            "\n",
            "    public function bye()\n",
            "    {\n",
            "        run();\n",
            "    }\n",
            "}\n",
            "\n",
            "function helper()\n",
            "{\n",
            "    return 1;\n",
            "}\n",
        ),
    );

    // `namespace X;` has no braces but scopes the rest of the file.
    let namespace = span(&conn, "package", "App\\Models");
    assert_eq!(namespace, (2, 22));
    let class = span(&conn, "class", "User");
    assert_eq!(class, (6, 17));
    assert_encloses(namespace, class);
    assert_eq!(span(&conn, "function", "greet"), (8, 11));
    assert_eq!(span(&conn, "function", "bye"), (13, 16));
    assert_encloses(class, span(&conn, "function", "greet"));
    assert_encloses(class, span(&conn, "function", "bye"));
    assert_eq!(span(&conn, "function", "helper"), (19, 22));
    assert_all_ranges_filled(&conn);
}

#[test]
fn php_statement_namespace_ends_before_the_next_one() {
    let conn = index_single(
        "src/two.php",
        "<?php\nnamespace A;\nclass X {}\nnamespace B;\nclass Y {\n}\n",
    );

    assert_eq!(span(&conn, "package", "A"), (2, 3));
    assert_eq!(span(&conn, "package", "B"), (4, 6));
}

#[test]
fn scala_class_object_and_methods_get_ranges() {
    let conn = index_single(
        "src/main/scala/x/Greeter.scala",
        concat!(
            "package x\n",
            "\n",
            "class Greeter(name: String) extends Base {\n",
            "  val count = 0\n",
            "\n",
            "  def hello(): String = {\n",
            "    greet()\n",
            "  }\n",
            "\n",
            "  def bye(): Unit =\n",
            "    run()\n",
            "}\n",
            "\n",
            "object Greeter {\n",
            "  def apply(): Greeter = new Greeter(\"x\")\n",
            "}\n",
            "\n",
            "def top(): Int =\n",
            "  1\n",
        ),
    );

    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (3, 12));
    assert_eq!(span(&conn, "function", "hello"), (6, 8));
    assert_eq!(span(&conn, "function", "bye"), (10, 11));
    assert_encloses(class, span(&conn, "function", "hello"));
    assert_encloses(class, span(&conn, "function", "bye"));
    let object = span(&conn, "object", "Greeter");
    assert_eq!(object, (14, 16));
    assert_encloses(object, span(&conn, "function", "apply"));
    assert_eq!(span(&conn, "function", "top"), (18, 19));
    assert_all_ranges_filled(&conn);
}

#[test]
fn dart_class_methods_and_function_get_ranges() {
    let conn = index_single(
        "lib/greeter.dart",
        concat!(
            "import 'base.dart';\n",
            "\n",
            "class Greeter extends Base {\n",
            "  final int count = 0;\n",
            "\n",
            "  String hello() {\n",
            "    return greet();\n",
            "  }\n",
            "\n",
            "  void bye() {\n",
            "    run();\n",
            "  }\n",
            "}\n",
            "\n",
            "void top() {\n",
            "  void local() {\n",
            "    run();\n",
            "  }\n",
            "}\n",
        ),
    );

    let class = span(&conn, "class", "Greeter");
    assert_eq!(class, (3, 13));
    // The body is a sibling of the signature in the grammar.
    assert_eq!(span(&conn, "function", "hello"), (6, 8));
    assert_eq!(span(&conn, "function", "bye"), (10, 12));
    assert_encloses(class, span(&conn, "function", "hello"));
    assert_encloses(class, span(&conn, "function", "bye"));
    let top = span(&conn, "function", "top");
    assert_eq!(top, (15, 19));
    assert_encloses(top, span(&conn, "function", "local"));
    assert_all_ranges_filled(&conn);
}
