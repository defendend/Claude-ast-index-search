//! Tree-sitter based R parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static R_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_r::LANGUAGE.into());

static R_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&R_LANGUAGE, include_str!("queries/r.scm"))
        .expect("Failed to compile R tree-sitter query")
});

pub static R_PARSER: RParser = RParser;

pub struct RParser;

impl LanguageParser for RParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &R_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*R_QUERY;

        // Build capture name → index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_func_name_arrow = idx("func_name_arrow");
        let idx_func_name_equals = idx("func_name_equals");
        let idx_func_name_global = idx("func_name_global");
        let idx_import_library_name = idx("import_library_name");
        let idx_import_require_name = idx("import_require_name");
        let idx_import_library_str = idx("import_library_str");
        let idx_import_require_str = idx("import_require_str");
        let idx_s4_class_name = idx("s4_class_name");
        let idx_s4_method_name = idx("s4_method_name");
        let idx_r6_class_name = idx("r6_class_name");
        let idx_r6_class_name_eq = idx("r6_class_name_eq");
        let idx_s4_generic_name = idx("s4_generic_name");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Function with <- operator
            if let Some(cap) = find_capture(m, idx_func_name_arrow) {
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

            // Function with = operator
            if let Some(cap) = find_capture(m, idx_func_name_equals) {
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

            // Function with <<- operator
            if let Some(cap) = find_capture(m, idx_func_name_global) {
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

            // R6Class with <-
            if let Some(cap) = find_capture(m, idx_r6_class_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // R6Class with =
            if let Some(cap) = find_capture(m, idx_r6_class_name_eq) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // S4 class
            if let Some(cap) = find_capture(m, idx_s4_class_name) {
                let raw = node_text(content, &cap.node);
                let name = raw.trim_matches('"').trim_matches('\'');
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // S4 method
            if let Some(cap) = find_capture(m, idx_s4_method_name) {
                let raw = node_text(content, &cap.node);
                let name = raw.trim_matches('"').trim_matches('\'');
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

            // S4 generic
            if let Some(cap) = find_capture(m, idx_s4_generic_name) {
                let raw = node_text(content, &cap.node);
                let name = raw.trim_matches('"').trim_matches('\'');
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

            // library() import — identifier
            if let Some(cap) = find_capture(m, idx_import_library_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // require() import — identifier
            if let Some(cap) = find_capture(m, idx_import_require_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // library() with string
            if let Some(cap) = find_capture(m, idx_import_library_str) {
                let raw = node_text(content, &cap.node);
                let name = raw.trim_matches('"').trim_matches('\'');
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // require() with string
            if let Some(cap) = find_capture(m, idx_import_require_str) {
                let raw = node_text(content, &cap.node);
                let name = raw.trim_matches('"').trim_matches('\'');
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
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
    fn test_function_assignment() {
        let content = "my_func <- function(x, y) {\n  x + y\n}\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "my_func" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_function_arrow() {
        let content = "add = function(a, b) a + b\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "add" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_library_call() {
        let content = "library(dplyr)\nrequire(ggplot2)\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "dplyr" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ggplot2" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "# fake <- function() {}\nreal <- function() { 1 }\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "real" && s.kind == SymbolKind::Function));
        assert!(!symbols.iter().any(|s| s.name == "fake"));
    }

    #[test]
    fn test_full_script() {
        let content = r#"# Load packages
library(dplyr)
require(ggplot2)

# Define functions
process_data <- function(df) {
  df %>% filter(!is.na(value))
}

compute_mean = function(x) {
  mean(x, na.rm = TRUE)
}

# S4 class
setClass("MyModel", representation(
  data = "data.frame",
  params = "list"
))

setMethod("show", "MyModel", function(object) {
  cat("MyModel\n")
})

# R6 class
MyService <- R6Class("MyService",
  public = list(
    initialize = function(name) {
      self$name <- name
    }
  )
)

setGeneric("fit", function(object, ...) standardGeneric("fit"))
"#;
        let symbols = R_PARSER.parse_symbols(content).unwrap();

        // Imports
        assert!(symbols
            .iter()
            .any(|s| s.name == "dplyr" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ggplot2" && s.kind == SymbolKind::Import));

        // Functions
        assert!(symbols
            .iter()
            .any(|s| s.name == "process_data" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "compute_mean" && s.kind == SymbolKind::Function));

        // S4 class
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyModel" && s.kind == SymbolKind::Class));

        // S4 method
        assert!(symbols
            .iter()
            .any(|s| s.name == "show" && s.kind == SymbolKind::Function));

        // R6 class
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyService" && s.kind == SymbolKind::Class));

        // S4 generic
        assert!(symbols
            .iter()
            .any(|s| s.name == "fit" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_s4_class() {
        let content =
            "setClass(\"Person\", representation(name = \"character\", age = \"numeric\"))\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Person" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_r6_class() {
        let content =
            "Animal <- R6Class(\"Animal\", public = list(speak = function() cat(\"...\")))\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Animal" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_global_assignment_function() {
        let content = "global_fn <<- function(x) x * 2\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "global_fn" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_library_string_argument() {
        let content = "library(\"tidyverse\")\nrequire(\"data.table\")\n";
        let symbols = R_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "tidyverse" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "data.table" && s.kind == SymbolKind::Import));
    }
}
