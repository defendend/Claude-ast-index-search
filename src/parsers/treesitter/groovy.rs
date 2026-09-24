//! Tree-sitter based Groovy parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static GROOVY_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_groovy::LANGUAGE.into());

static GROOVY_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&GROOVY_LANGUAGE, include_str!("queries/groovy.scm"))
        .expect("Failed to compile Groovy tree-sitter query")
});

pub static GROOVY_PARSER: GroovyParser = GroovyParser;

pub struct GroovyParser;

/// Comments and string and character literals.
static NON_CODE: super::NonCode = super::NonCode {
    language: &GROOVY_LANGUAGE,
    prose: &["line_comment", "block_comment"],
    strings: &["string_literal", "character_literal"],
    code: &[],
    keep: gstring,
};

/// A double-quoted string with `$` in it, kept whole: the grammar reads a
/// GString's `${expr}` as plain text, and the expression is code.
fn gstring(content: &str, string: tree_sitter::Node) -> Vec<std::ops::Range<usize>> {
    let text = node_text(content, &string);
    if text.starts_with('"') && text.contains('$') {
        vec![string.byte_range()]
    } else {
        Vec::new()
    }
}

impl LanguageParser for GroovyParser {
    fn non_code(&self) -> Option<&'static super::NonCode> {
        Some(&NON_CODE)
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &GROOVY_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*GROOVY_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_package_name = idx("package_name");
        let idx_import_path = idx("import_path");
        let idx_class_name = idx("class_name");
        let idx_interface_name = idx("interface_name");
        let idx_enum_name = idx("enum_name");
        let idx_method_name = idx("method_name");
        let idx_constructor_name = idx("constructor_name");
        let idx_field_name = idx("field_name");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Package
            if let Some(cap) = find_capture(m, idx_package_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Import
            if let Some(cap) = find_capture(m, idx_import_path) {
                let full_path = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                // Extract the last component as the name
                let name = full_path.rsplit('.').next().unwrap_or(full_path);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![(full_path.to_string(), "from".to_string())],
                    end_line,
                });
                continue;
            }

            // Class
            if let Some(cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Interface
            if let Some(cap) = find_capture(m, idx_interface_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Enum
            if let Some(cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Method
            if let Some(cap) = find_capture(m, idx_method_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Constructor
            if let Some(cap) = find_capture(m, idx_constructor_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Field / Variable
            if let Some(cap) = find_capture(m, idx_field_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Find a capture by index in a match
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
    fn test_parse_class() {
        let content = "class MyService {\n}\n";
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyService" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_method() {
        let content = r#"class MyService {
    void processData(String input) {
    }
}
"#;
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "processData" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_function() {
        let content = r#"class Utils {
    String formatName(String first, String last) {
        return first + " " + last;
    }
}
"#;
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "formatName" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_variable() {
        let content = r#"class Config {
    String apiUrl;
    int maxRetries;
}
"#;
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "apiUrl" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "maxRetries" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_interface() {
        let content = "interface Repository {\n    List getAll();\n}\n";
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Repository" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// class FakeClass {}\nclass RealClass {}\n/* void fakeMethod() {} */\n";
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(!symbols.iter().any(|s| s.name == "fakeMethod"));
    }

    #[test]
    fn test_parse_enum() {
        let content = "enum Status {\n    ACTIVE,\n    INACTIVE\n}\n";
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_import() {
        let content = "import java.util.List;\n";
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "List" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_package() {
        let content = "package com.example.app;\n\nclass MyApp {}\n";
        let symbols = GROOVY_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Package));
    }
}
