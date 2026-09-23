//! Tree-sitter based PHP parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static PHP_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_php::LANGUAGE_PHP.into());

static PHP_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&PHP_LANGUAGE, include_str!("queries/php.scm"))
        .expect("Failed to compile PHP tree-sitter query")
});

pub static PHP_PARSER: PhpParser = PhpParser;

pub struct PhpParser;

impl LanguageParser for PhpParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &PHP_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*PHP_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_namespace_name = idx("namespace_name");
        let idx_class_name = idx("class_name");
        let idx_class_parent = idx("class_parent");
        let idx_class_interface = idx("class_interface");
        let idx_interface_name = idx("interface_name");
        let idx_interface_parent = idx("interface_parent");
        let idx_trait_name = idx("trait_name");
        let idx_enum_name = idx("enum_name");
        let idx_func_name = idx("func_name");
        let idx_method_name = idx("method_name");
        let idx_const_name = idx("const_name");
        let idx_prop_name = idx("prop_name");
        let idx_use_name = idx("use_name");
        let idx_use_simple_name = idx("use_simple_name");
        let idx_trait_use_qualified = idx("trait_use_qualified");
        let idx_trait_use_name = idx("trait_use_name");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Namespace
            if let Some(cap) = find_capture(m, idx_namespace_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let end_line = cap
                    .node
                    .parent()
                    .filter(|decl| decl.child_by_field_name("body").is_none())
                    .map(|decl| statement_namespace_end_line(content, &decl))
                    .or(end_line);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Class
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                let mut parents = Vec::new();
                if let Some(parent_cap) = find_capture(m, idx_class_parent) {
                    let parent = node_text(content, &parent_cap.node);
                    parents.push((parent.to_string(), "extends".to_string()));
                }
                if let Some(iface_cap) = find_capture(m, idx_class_interface) {
                    let iface = node_text(content, &iface_cap.node);
                    parents.push((iface.to_string(), "implements".to_string()));
                }
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line,
                });
                continue;
            }

            // Interface
            if let Some(name_cap) = find_capture(m, idx_interface_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                let parents = find_capture(m, idx_interface_parent)
                    .map(|p| {
                        vec![(
                            node_text(content, &p.node).to_string(),
                            "extends".to_string(),
                        )]
                    })
                    .unwrap_or_default();
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line,
                });
                continue;
            }

            // Trait
            if let Some(cap) = find_capture(m, idx_trait_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: line_text(content, line).trim().to_string(),
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
                    kind: SymbolKind::Enum,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Function (top-level)
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

            // Method
            if let Some(cap) = find_capture(m, idx_method_name) {
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

            // Constant
            if let Some(cap) = find_capture(m, idx_const_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Constant,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Property
            if let Some(cap) = find_capture(m, idx_prop_name) {
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

            // Namespace use (import) — qualified or simple name
            if let Some(cap) =
                find_capture(m, idx_use_name).or_else(|| find_capture(m, idx_use_simple_name))
            {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Trait use inside class — qualified or simple name
            if let Some(cap) = find_capture(m, idx_trait_use_qualified)
                .or_else(|| find_capture(m, idx_trait_use_name))
            {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
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

/// `namespace A\B;` has no body: it scopes everything up to the next namespace
/// statement or the end of the file, while the grammar node stops at the `;`.
fn statement_namespace_end_line(content: &str, decl: &tree_sitter::Node) -> usize {
    let mut sibling = decl.next_sibling();
    while let Some(next) = sibling {
        if next.kind() == "namespace_definition" {
            return node_line(&next).saturating_sub(1).max(node_line(decl));
        }
        sibling = next.next_sibling();
    }
    decl.parent()
        .map(|program| text_end_line(content, &program))
        .unwrap_or_else(|| text_end_line(content, decl))
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
    fn test_parse_namespace() {
        let content = "<?php\nnamespace App\\Models;\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "App\\Models" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_class() {
        let content = "<?php\nclass User {\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_class_extends() {
        let content = "<?php\nclass User extends Model {\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "User" && s.kind == SymbolKind::Class);
        assert!(cls.is_some());
        assert!(cls
            .unwrap()
            .parents
            .iter()
            .any(|(p, k)| p == "Model" && k == "extends"));
    }

    #[test]
    fn test_parse_class_implements() {
        let content = "<?php\nclass User extends Model implements Authenticatable {\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "User" && s.kind == SymbolKind::Class);
        assert!(cls.is_some());
        assert!(cls
            .unwrap()
            .parents
            .iter()
            .any(|(p, k)| p == "Model" && k == "extends"));
        assert!(cls.unwrap().parents.iter().any(|(_, k)| k == "implements"));
    }

    #[test]
    fn test_parse_interface() {
        let content =
            "<?php\ninterface Authenticatable {\n    public function getAuthIdentifier();\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Authenticatable" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_interface_extends() {
        let content = "<?php\ninterface AdminAuth extends Authenticatable {\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        let iface = symbols
            .iter()
            .find(|s| s.name == "AdminAuth" && s.kind == SymbolKind::Interface);
        assert!(iface.is_some());
        assert!(iface
            .unwrap()
            .parents
            .iter()
            .any(|(p, k)| p == "Authenticatable" && k == "extends"));
    }

    #[test]
    fn test_parse_trait() {
        let content =
            "<?php\ntrait HasFactory {\n    public function factory() { return new static; }\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "HasFactory" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_enum() {
        let content = "<?php\nenum Status {\n    case Active;\n    case Inactive;\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_function() {
        let content = "<?php\nfunction helper(): string {\n    return 'hello';\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "helper" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_method() {
        let content = "<?php\nclass User {\n    public function getName(): string {\n        return $this->name;\n    }\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "getName" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_constant() {
        let content =
            "<?php\nclass Config {\n    const MAX_RETRIES = 3;\n    const VERSION = '1.0';\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "VERSION" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_property() {
        let content = "<?php\nclass User {\n    public string $name;\n    protected int $age;\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "$name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "$age" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_use_import() {
        let content = "<?php\nuse App\\Models\\User;\nuse Illuminate\\Support\\Facades\\DB;\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "App\\Models\\User" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Illuminate\\Support\\Facades\\DB" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_trait_use() {
        let content = "<?php\nclass User {\n    use HasFactory;\n    use Notifiable;\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "HasFactory" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Notifiable" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_comments_ignored() {
        let content =
            "<?php\n// class FakeClass {}\n/* class AnotherFake {} */\nclass RealClass {\n}\n";
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(!symbols.iter().any(|s| s.name == "AnotherFake"));
    }

    #[test]
    fn test_parse_full_laravel_model() {
        let content = r#"<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Contracts\Auth\Authenticatable;

class User extends Model implements Authenticatable {
    use HasFactory;
    use Notifiable;

    const TABLE = 'users';

    public string $name;
    protected string $email;

    public function getName(): string {
        return $this->name;
    }

    public static function findByEmail(string $email): ?self {
        return static::where('email', $email)->first();
    }
}
"#;
        let symbols = PHP_PARSER.parse_symbols(content).unwrap();

        // Namespace
        assert!(symbols
            .iter()
            .any(|s| s.name == "App\\Models" && s.kind == SymbolKind::Package));

        // Imports
        assert!(symbols
            .iter()
            .any(|s| s.name == "Illuminate\\Database\\Eloquent\\Model"
                && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Illuminate\\Contracts\\Auth\\Authenticatable"
                && s.kind == SymbolKind::Import));

        // Class
        let cls = symbols
            .iter()
            .find(|s| s.name == "User" && s.kind == SymbolKind::Class);
        assert!(cls.is_some());

        // Trait use
        assert!(symbols
            .iter()
            .any(|s| s.name == "HasFactory" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Notifiable" && s.kind == SymbolKind::Import));

        // Constant
        assert!(symbols
            .iter()
            .any(|s| s.name == "TABLE" && s.kind == SymbolKind::Constant));

        // Properties
        assert!(symbols
            .iter()
            .any(|s| s.name == "$name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "$email" && s.kind == SymbolKind::Property));

        // Methods
        assert!(symbols
            .iter()
            .any(|s| s.name == "getName" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "findByEmail" && s.kind == SymbolKind::Function));
    }
}
