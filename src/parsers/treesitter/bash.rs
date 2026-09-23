//! Tree-sitter based Bash parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static BASH_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_bash::LANGUAGE.into());

static BASH_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&BASH_LANGUAGE, include_str!("queries/bash.scm"))
        .expect("Failed to compile Bash tree-sitter query")
});

pub static BASH_PARSER: BashParser = BashParser;

pub struct BashParser;

impl LanguageParser for BashParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &BASH_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*BASH_QUERY;

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_func_name = idx("func_name");
        let idx_var_name = idx("var_name");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Function definition
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Variable assignment
            if let Some(cap) = find_capture(m, idx_var_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
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
    fn test_function_keyword() {
        let content = "function greet {\n    echo \"Hello\"\n}\n";
        let symbols = BASH_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "greet" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_function_parens() {
        let content = "greet() {\n    echo \"Hello\"\n}\n";
        let symbols = BASH_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "greet" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_variable() {
        let content = "MY_VAR=\"hello world\"\nCOUNT=42\n";
        let symbols = BASH_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MY_VAR" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "COUNT" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "# function fake {\n# }\nfunction real {\n    echo ok\n}\n# MY_VAR=nope\nREAL_VAR=yes\n";
        let symbols = BASH_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "real" && s.kind == SymbolKind::Function));
        assert!(!symbols.iter().any(|s| s.name == "fake"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "REAL_VAR" && s.kind == SymbolKind::Property));
        assert!(!symbols.iter().any(|s| s.name == "MY_VAR"));
    }

    #[test]
    fn test_function_with_keyword_and_parens() {
        let content = "function deploy() {\n    echo \"deploying\"\n}\n";
        let symbols = BASH_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "deploy" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_multiple_symbols() {
        let content = r#"#!/bin/bash

MY_APP="myapp"
VERSION="1.0"

function start {
    echo "starting"
}

stop() {
    echo "stopping"
}
"#;
        let symbols = BASH_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MY_APP" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "VERSION" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "start" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "stop" && s.kind == SymbolKind::Function));
    }
}
