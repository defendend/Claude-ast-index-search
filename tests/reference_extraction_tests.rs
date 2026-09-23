//! What the generic reference extractor records: snake_case calls in every
//! language that goes through it, and never a reserved word.

use std::fs;
use std::path::Path;

use ast_index::parsers::{parse_file_symbols, FileType};
use ast_index::{db, indexer};
use rusqlite::Connection;
use tempfile::TempDir;

fn ref_lines(content: &str, file_type: FileType, name: &str) -> Vec<usize> {
    let (_, refs) = parse_file_symbols(content, file_type).unwrap();
    refs.iter()
        .filter(|r| r.name == name)
        .map(|r| r.line)
        .collect()
}

fn ref_names(content: &str, file_type: FileType) -> Vec<String> {
    let (_, refs) = parse_file_symbols(content, file_type).unwrap();
    refs.into_iter().map(|r| r.name).collect()
}

#[test]
fn snake_case_calls_are_references_in_every_generic_language() {
    let cases: [(FileType, &str); 6] = [
        (
            FileType::Python,
            "def run(user):\n    update_profile(user)\n    user.update_profile(1)\n",
        ),
        (
            FileType::Rust,
            "fn run(u: &User) {\n    update_profile(u);\n    u.update_profile(1);\n}\n",
        ),
        (
            FileType::Go,
            "package main\n\nfunc run(u *User) {\n\tupdate_profile(u)\n\tu.update_profile(1)\n}\n",
        ),
        (
            FileType::Cpp,
            "void run(struct user *u) {\n    update_profile(u);\n    u->update_profile(1);\n}\n",
        ),
        (
            FileType::Php,
            "<?php\nfunction run($u) {\n    update_profile($u);\n    $u->update_profile(1);\n}\n",
        ),
        (
            FileType::Lua,
            "local function run(u)\n  update_profile(u)\n  u:update_profile(1)\nend\n",
        ),
    ];
    for (file_type, content) in cases {
        let lines = ref_lines(content, file_type, "update_profile");
        assert_eq!(
            lines.len(),
            2,
            "{file_type:?}: expected both update_profile calls, got lines {lines:?}"
        );
        let names = ref_names(content, file_type);
        assert!(
            !names.iter().any(|n| n == "profile" || n == "update"),
            "{file_type:?}: a snake_case name must not be split, got {names:?}"
        );
    }
}

#[test]
fn leading_underscore_calls_are_references() {
    let content = "class Box:\n    def open(self):\n        return self._compute(1) + _helper()\n";
    assert_eq!(ref_lines(content, FileType::Python, "_compute"), vec![3]);
    assert_eq!(ref_lines(content, FileType::Python, "_helper"), vec![3]);
}

#[test]
fn a_definition_line_is_not_a_reference_to_itself() {
    let content = "def update_profile(user):\n    pass\n\nupdate_profile(None)\n";
    assert_eq!(
        ref_lines(content, FileType::Python, "update_profile"),
        vec![4]
    );
}

#[test]
fn reserved_words_before_a_parenthesis_are_not_references() {
    let cases: [(FileType, &str, &[&str]); 6] = [
        (
            FileType::Python,
            "def f(x):\n    try:\n        assert (x)\n    except (KeyError, ValueError):\n        return not (x) and (None)\n",
            &["assert", "except", "not", "and", "None"],
        ),
        (
            FileType::Cpp,
            "#if defined(USE_SSE) && !defined(NDEBUG)\nint (*cb)(void);\n#endif\nint f(int x) {\n    switch (x) { case 1: return sizeof (x); }\n    return int(x);\n}\n",
            &["defined", "int", "switch", "sizeof"],
        ),
        (
            FileType::Go,
            "package main\n\nfunc (s *Server) Run() {\n\tgo func() {}()\n\tswitch (s.state) {\n\t}\n}\n",
            &["func", "switch"],
        ),
        (
            FileType::Rust,
            "pub(crate) fn f(p: (u8, u8)) -> Self {\n    let (a, b) = p;\n    match (a, b) { _ => Self::new() }\n}\n",
            &["pub", "let", "match", "Self"],
        ),
        (
            FileType::TypeScript,
            "export function f(x: number) {\n  switch (x) {}\n  return typeof (x);\n}\n",
            &["switch", "typeof"],
        ),
        (
            FileType::Perl,
            "sub f {\n    local($x) = @_;\n    foreach (@list) { }\n    if ($x) { } elsif ($x) { }\n}\n",
            &["local", "foreach", "elsif"],
        ),
    ];
    for (file_type, content, reserved) in cases {
        let names = ref_names(content, file_type);
        for word in reserved {
            assert!(
                !names.iter().any(|n| n == word),
                "{file_type:?}: reserved word {word:?} recorded as a reference: {names:?}"
            );
        }
    }
}

#[test]
fn reserved_words_used_as_member_or_sub_names_stay_references() {
    // JavaScript allows reserved words as property names: `Map#delete`.
    let js = "const seen = new Map();\nseen.delete(key);\n";
    assert_eq!(ref_lines(js, FileType::TypeScript, "delete"), vec![2]);

    // `&name(...)` always calls a sub in Perl; perlasm is written this way.
    let perl = "sub body {\n    &xor(\"eax\", \"ebx\");\n    &sub(\"ecx\", 16);\n}\n";
    assert_eq!(ref_lines(perl, FileType::Perl, "xor"), vec![2]);
    assert_eq!(ref_lines(perl, FileType::Perl, "sub"), vec![3]);
}

#[test]
fn contextual_keywords_that_name_functions_stay_references() {
    let content = "import re\n\ndef f(text):\n    return re.match(r'x', text) or type(text)\n";
    assert_eq!(ref_lines(content, FileType::Python, "match"), vec![4]);
    assert_eq!(ref_lines(content, FileType::Python, "type"), vec![4]);
}

#[test]
fn ruby_lowercase_calls_come_only_from_code() {
    let content = "class Account
  # Account.update_profile(attrs) is the public entry point.
  def self.build_profile(attrs)
    update_profile(attrs)
    sql = \"SELECT pg_get_expr(x) FROM y\"
    attrs.each_with_object({}) { |pair, memo| memo }
  end
end
";
    assert_eq!(
        ref_lines(content, FileType::Ruby, "update_profile"),
        vec![4]
    );
    // A core method called with parentheses is recorded, as it always was
    // for single-word names like `map(`; only calls without them are not.
    assert_eq!(
        ref_lines(content, FileType::Ruby, "each_with_object"),
        vec![6]
    );
    let names = ref_names(content, FileType::Ruby);
    for noise in ["build_profile", "pg_get_expr"] {
        assert!(
            !names.iter().any(|n| n == noise),
            "{noise:?} recorded as a Ruby reference: {names:?}"
        );
    }
}

fn open_fresh_db(project_root: &Path) -> Connection {
    if db::db_exists(project_root) {
        db::delete_db(project_root).unwrap();
    }
    let conn = db::open_db(project_root).unwrap();
    db::init_db(&conn).unwrap();
    conn
}

#[test]
fn usages_of_a_snake_case_function_are_indexed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(
        root.join("pkg/paths.py"),
        "def is_third_party_path(path):\n    return 'site-packages/' in path\n",
    )
    .unwrap();
    fs::write(
        root.join("pkg/rank.py"),
        "from pkg.paths import is_third_party_path\n\n\ndef demote(paths):\n    return [p for p in paths if is_third_party_path(p)]\n",
    )
    .unwrap();
    fs::write(
        root.join("src.rs"),
        "fn keep(path: &str) -> bool {\n    !paths::is_third_party_path(path)\n}\n",
    )
    .unwrap();

    let mut conn = open_fresh_db(root);
    indexer::index_directory(&mut conn, root, false, false).unwrap();

    let refs = db::find_references(&conn, "is_third_party_path", 50).unwrap();
    let hits: Vec<(String, i64)> = refs.iter().map(|r| (r.path.clone(), r.line)).collect();
    for expected in [("pkg/rank.py", 5i64), ("src.rs", 2i64)] {
        assert!(
            hits.contains(&(expected.0.to_string(), expected.1)),
            "missing usage at {expected:?}; got {hits:?}"
        );
    }
    assert!(
        !hits.contains(&("pkg/paths.py".to_string(), 1)),
        "the definition is not a usage: {hits:?}"
    );
}
