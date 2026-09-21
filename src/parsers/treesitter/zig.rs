//! Tree-sitter based Zig parser.
//!
//! Zig's grammar has no dedicated `struct`/`enum`/`union` keywords at the
//! statement level — containers are expressions assigned to `const`, e.g.
//! `pub const Foo = struct { ... };`. We therefore emit every `VarDecl`
//! as a single symbol (kind=Constant) regardless of what its initializer
//! is; the signature line preserves the `struct` / `enum` / `union`
//! hint for the human reader.

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static ZIG_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_zig::LANGUAGE.into());

static ZIG_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&ZIG_LANGUAGE, include_str!("queries/zig.scm"))
        .expect("Failed to compile Zig tree-sitter query")
});

pub static ZIG_PARSER: ZigParser = ZigParser;

pub struct ZigParser;

impl LanguageParser for ZigParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &ZIG_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*ZIG_QUERY;

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_func_name = idx("func_name");
        let idx_var_name = idx("var_name");
        let idx_test_name = idx("test_name");
        let idx_test_ident = idx("test_ident");
        let idx_field_name = idx("field_name");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Function declaration
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // Const / var declaration — also captures type definitions like
            // `const Foo = struct { ... };` since Zig expresses types as
            // expressions.
            if let Some(cap) = find_capture(m, idx_var_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let sig_line = line_text(content, line).trim();
                let kind = classify_var(sig_line);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind,
                    line,
                    signature: sig_line.to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // `test "does X" { ... }` — store the quoted name minus quotes
            if let Some(cap) = find_capture(m, idx_test_name) {
                let raw = node_text(content, &cap.node);
                let name = raw.trim_matches('"');
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: format!("test {}", name),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // `test identifier { ... }` — bare-identifier form
            if let Some(cap) = find_capture(m, idx_test_ident) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: format!("test {}", name),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // Struct / enum / union field
            if let Some(cap) = find_capture(m, idx_field_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Heuristic: classify a `const`/`var` declaration by looking at the same line.
/// Zig's `const Foo = struct { ... }` looks like a constant syntactically but
/// carries more useful information as a type. Multi-line initializers where the
/// `struct`/`enum`/`union` keyword sits on a later line will fall through to
/// `Constant` — acceptable, since the symbol is still found by name.
fn classify_var(signature: &str) -> SymbolKind {
    if signature.contains("= struct") || signature.contains("=struct") {
        SymbolKind::Class
    } else if signature.contains("= enum") || signature.contains("=enum") {
        SymbolKind::Enum
    } else if signature.contains("= union") || signature.contains("=union") {
        SymbolKind::Class
    } else {
        SymbolKind::Constant
    }
}

fn find_capture<'a>(
    m: &'a tree_sitter::QueryMatch<'a, 'a>,
    idx: Option<u32>,
) -> Option<&'a tree_sitter::QueryCapture<'a>> {
    let idx = idx?;
    m.captures.iter().find(|c| c.index == idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_function() {
        let content = "pub fn add(a: i32, b: i32) i32 {\n    return a + b;\n}\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "add" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_struct() {
        let content = "pub const Point = struct {\n    x: f32,\n    y: f32,\n};\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Point" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "x" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "y" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_enum() {
        let content = "pub const Color = enum {\n    red,\n    green,\n    blue,\n};\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Color" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_constant() {
        let content = "pub const MAX_SIZE: usize = 1024;\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_SIZE" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_import_const() {
        // @import is just a builtin call assigned to a const — we track the
        // binding name, since that is what gets referenced elsewhere.
        let content = "const std = @import(\"std\");\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "std"));
    }

    #[test]
    fn test_parse_test_block() {
        let content = "test \"basic addition\" {\n    try std.testing.expect(1 + 1 == 2);\n}\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "test basic addition" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// const FakeConst = 5;\nconst RealConst = 7;\n// fn fake_fn() void {}\nfn real_fn() void {}\n";
        let symbols = ZIG_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealConst"));
        assert!(!symbols.iter().any(|s| s.name == "FakeConst"));
        assert!(symbols.iter().any(|s| s.name == "real_fn"));
        assert!(!symbols.iter().any(|s| s.name == "fake_fn"));
    }
}
