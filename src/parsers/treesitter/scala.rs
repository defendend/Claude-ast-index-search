//! Tree-sitter based Scala parser

use anyhow::Result;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};
use std::sync::LazyLock;

use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;
use super::{LanguageParser, parse_tree, node_text, node_line, line_text};

static SCALA_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_scala::LANGUAGE.into());

static SCALA_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&SCALA_LANGUAGE, include_str!("queries/scala.scm"))
        .expect("Failed to compile Scala tree-sitter query")
});

pub static SCALA_PARSER: ScalaParser = ScalaParser;

pub struct ScalaParser;

impl LanguageParser for ScalaParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &SCALA_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*SCALA_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names.iter().position(|n| *n == name).map(|i| i as u32)
        };

        let idx_class_name = idx("class_name");
        let idx_class_decl = idx("class_decl");
        let idx_object_name = idx("object_name");
        let idx_object_decl = idx("object_decl");
        let idx_trait_name = idx("trait_name");
        let idx_trait_decl = idx("trait_decl");
        let idx_enum_name = idx("enum_name");
        let idx_func_name = idx("func_name");
        let idx_func_decl_name = idx("func_decl_name");
        let idx_val_name = idx("val_name");
        let idx_val_decl_name = idx("val_decl_name");
        let idx_var_name = idx("var_name");
        let idx_var_decl_name = idx("var_decl_name");
        let idx_type_name = idx("type_name");
        let idx_given_name = idx("given_name");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Class definition
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let parents = if let Some(decl) = find_capture(m, idx_class_decl) {
                    parse_extends_clause(&decl.node, content)
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                });
                continue;
            }

            // Object definition
            if let Some(name_cap) = find_capture(m, idx_object_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let parents = if let Some(decl) = find_capture(m, idx_object_decl) {
                    parse_extends_clause(&decl.node, content)
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Object,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                });
                continue;
            }

            // Trait definition
            if let Some(name_cap) = find_capture(m, idx_trait_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let parents = if let Some(decl) = find_capture(m, idx_trait_decl) {
                    parse_extends_clause(&decl.node, content)
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                });
                continue;
            }

            // Enum definition (Scala 3)
            if let Some(name_cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Function definition (def foo = ...)
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Function declaration (abstract def)
            if let Some(cap) = find_capture(m, idx_func_decl_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Val definition/declaration
            if let Some(cap) = find_capture(m, idx_val_name)
                .or_else(|| find_capture(m, idx_val_decl_name))
            {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Var definition/declaration
            if let Some(cap) = find_capture(m, idx_var_name)
                .or_else(|| find_capture(m, idx_var_decl_name))
            {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Type alias
            if let Some(cap) = find_capture(m, idx_type_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::TypeAlias,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Given definition (Scala 3)
            if let Some(cap) = find_capture(m, idx_given_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Object,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

        }

        Ok(symbols)
    }
}

/// Parse extends_clause from a class/trait/object definition node.
/// Looks for extends_clause child, then extracts type names.
fn parse_extends_clause(
    decl_node: &tree_sitter::Node,
    content: &str,
) -> Vec<(String, String)> {
    let mut parents = Vec::new();

    let mut walker = decl_node.walk();
    for child in decl_node.children(&mut walker) {
        if child.kind() == "extends_clause" {
            let mut first = true;
            let mut ec_walker = child.walk();
            for ec_child in child.children(&mut ec_walker) {
                if let Some(name) = extract_type_identifier(&ec_child, content) {
                    let kind = if first { "extends" } else { "implements" };
                    parents.push((name, kind.to_string()));
                    first = false;
                }
            }
        }
    }

    parents
}

/// Extract a type identifier from a node, recursing into type nodes.
fn extract_type_identifier(node: &tree_sitter::Node, content: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node_text(content, node).to_string()),
        "generic_type" | "compound_type" | "infix_type" | "annotated_type"
        | "lazy_parameter_type" | "structural_type" | "existential_type" => {
            // Get the first type_identifier child
            let mut walker = node.walk();
            for child in node.children(&mut walker) {
                if child.kind() == "type_identifier" {
                    return Some(node_text(content, &child).to_string());
                }
                if let Some(name) = extract_type_identifier(&child, content) {
                    return Some(name);
                }
            }
            None
        }
        _ => None,
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
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MyService" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_case_class() {
        let content = "case class User(name: String, age: Int)\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_abstract_class() {
        let content = "abstract class Animal {\n  def speak(): String\n}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Animal" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "speak" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_object() {
        let content = "object Singleton {\n  val instance = 42\n}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Singleton" && s.kind == SymbolKind::Object));
    }

    #[test]
    fn test_parse_case_object() {
        let content = "case object Empty extends Option[Nothing]\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Empty" && s.kind == SymbolKind::Object));
    }

    #[test]
    fn test_parse_trait() {
        let content = "trait Repository[T] {\n  def findAll(): List[T]\n}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Repository" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_function() {
        let content = "def processPayment(amount: Double): Boolean = {\n  true\n}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "processPayment" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_val() {
        let content = "val name: String = \"test\"\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "name" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_var() {
        let content = "var count: Int = 0\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "count" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = "type StringMap = Map[String, String]\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "StringMap" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_class_with_extends() {
        let content = "class Dog(name: String) extends Animal with Serializable {\n}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "Dog" && s.kind == SymbolKind::Class).unwrap();
        assert!(cls.parents.iter().any(|(p, k)| p == "Animal" && k == "extends"));
        assert!(cls.parents.iter().any(|(p, k)| p == "Serializable" && k == "implements"));
    }

    #[test]
    fn test_parse_trait_extends() {
        let content = "trait Service extends Closeable with Logging {\n}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        let tr = symbols.iter().find(|s| s.name == "Service" && s.kind == SymbolKind::Interface).unwrap();
        assert!(!tr.parents.is_empty());
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// class FakeClass {}\nclass RealClass {}\n/* def fake() = {} */\ndef real(): Unit = {}\n";
        let symbols = SCALA_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(symbols.iter().any(|s| s.name == "real"));
        assert!(!symbols.iter().any(|s| s.name == "fake"));
    }
}
