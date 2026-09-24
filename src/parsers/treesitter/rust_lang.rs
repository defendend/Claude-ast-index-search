//! Tree-sitter based Rust parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{
    line_text, node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser,
};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static RUST_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_rust::LANGUAGE.into());

static RUST_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&RUST_LANGUAGE, include_str!("queries/rust.scm"))
        .expect("Failed to compile Rust tree-sitter query")
});

pub static RUST_PARSER: RustParser = RustParser;

pub struct RustParser;

/// Check if a name is ALL_CAPS (constant style)
fn is_all_caps(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_')
        && name.chars().any(|c| c.is_uppercase())
}

/// Check if an attribute is significant enough to track
fn is_significant_attr(name: &str) -> bool {
    matches!(
        name,
        "test"
            | "bench"
            | "cfg"
            | "allow"
            | "warn"
            | "deny"
            | "macro_export"
            | "inline"
            | "cold"
            | "must_use"
            | "tokio"
            | "async_trait"
            | "proc_macro"
            | "proc_macro_derive"
            | "serde"
            | "rocket"
            | "actix"
            | "axum"
    )
}

impl LanguageParser for RustParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &RUST_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*RUST_QUERY;

        // Build capture name -> index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_struct_name = idx("struct_name");
        let idx_enum_name = idx("enum_name");
        let idx_trait_name = idx("trait_name");
        let idx_impl_trait = idx("impl_trait");
        let idx_impl_trait_type = idx("impl_trait_type");
        let idx_impl_self_type = idx("impl_self_type");
        let idx_func_name = idx("func_name");
        let idx_func_sig_name = idx("func_sig_name");
        let idx_macro_name = idx("macro_name");
        let idx_type_alias_name = idx("type_alias_name");
        let idx_const_name = idx("const_name");
        let idx_static_name = idx("static_name");
        let idx_mod_name = idx("mod_name");
        let idx_use_path = idx("use_path");
        let idx_use_alias_path = idx("use_alias_path");
        let idx_attr = idx("attr");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Struct
            if let Some(cap) = find_capture(m, idx_struct_name) {
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

            // Enum
            if let Some(cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
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
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Impl Trait for Type
            if let Some(trait_cap) = find_capture(m, idx_impl_trait) {
                if let Some(type_cap) = find_capture(m, idx_impl_trait_type) {
                    let trait_name = node_text(content, &trait_cap.node);
                    let type_name = node_text(content, &type_cap.node);
                    let line = node_line(&trait_cap.node);
                    // Use the line of the impl keyword (parent node)
                    let impl_line = trait_cap
                        .node
                        .parent()
                        .map(|p| p.start_position().row + 1)
                        .unwrap_or(line);
                    symbols.push(ParsedSymbol {
                        name: format!("impl {} for {}", trait_name, type_name),
                        kind: SymbolKind::Class,
                        line: impl_line,
                        signature: signature_line(content, impl_line),
                        parents: vec![(trait_name.to_string(), "implements".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Impl Type (self impl)
            if let Some(cap) = find_capture(m, idx_impl_self_type) {
                let type_name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let impl_line = cap
                    .node
                    .parent()
                    .map(|p| p.start_position().row + 1)
                    .unwrap_or(line);
                symbols.push(ParsedSymbol {
                    name: format!("impl {}", type_name),
                    kind: SymbolKind::Class,
                    line: impl_line,
                    signature: signature_line(content, impl_line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Function (regular)
            if let Some(cap) = find_capture(m, idx_func_name) {
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

            // Function signature (trait method declaration)
            if let Some(cap) = find_capture(m, idx_func_sig_name) {
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

            // Macro definition
            if let Some(cap) = find_capture(m, idx_macro_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: format!("{}!", name),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Type alias
            if let Some(cap) = find_capture(m, idx_type_alias_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::TypeAlias,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Constant (only ALL_CAPS names)
            if let Some(cap) = find_capture(m, idx_const_name) {
                let name = node_text(content, &cap.node);
                if is_all_caps(name) {
                    let line = node_line(&cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Static (only ALL_CAPS names)
            if let Some(cap) = find_capture(m, idx_static_name) {
                let name = node_text(content, &cap.node);
                if is_all_caps(name) {
                    let line = node_line(&cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Module
            if let Some(cap) = find_capture(m, idx_mod_name) {
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

            // Use declaration (scoped path)
            if let Some(cap) = find_capture(m, idx_use_path) {
                let path = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: path.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Use declaration with alias (use X as Y)
            if let Some(cap) = find_capture(m, idx_use_alias_path) {
                let path = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: path.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Attributes (derive and other significant ones)
            if let Some(cap) = find_capture(m, idx_attr) {
                let attr_text = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let sig = line_text(content, line).trim().to_string();

                // Check if this is a derive attribute
                if attr_text.starts_with("derive(") || attr_text.starts_with("derive (") {
                    // Extract the content inside derive(...)
                    if let Some(start) = attr_text.find('(') {
                        if let Some(end) = attr_text.rfind(')') {
                            let derives = &attr_text[start + 1..end];
                            for derive in derives.split(',') {
                                let derive_name = derive.trim();
                                if !derive_name.is_empty() {
                                    symbols.push(ParsedSymbol {
                                        name: format!("#[derive({})]", derive_name),
                                        kind: SymbolKind::Annotation,
                                        line,
                                        signature: sig.clone(),
                                        parents: vec![],
                                        end_line,
                                    });
                                }
                            }
                        }
                    }
                } else {
                    // Other attributes: extract the attribute name (first identifier)
                    let attr_name = attr_text.split('(').next().unwrap_or(attr_text).trim();
                    if is_significant_attr(attr_name) {
                        symbols.push(ParsedSymbol {
                            name: format!("#[{}]", attr_name),
                            kind: SymbolKind::Annotation,
                            line,
                            signature: sig,
                            parents: vec![],
                            end_line,
                        });
                    }
                }
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
    fn test_parse_struct() {
        let content = "pub struct User {\n    pub id: u64,\n    pub name: String,\n}\n\nstruct PrivateData(i32);\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "PrivateData" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_enum() {
        let content = "pub enum Status {\n    Active,\n    Inactive,\n}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_trait() {
        let content = "pub trait Repository {\n    fn find(&self, id: u64) -> Option<User>;\n}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Repository" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_impl() {
        let content = "impl Repository for SqlUserRepository {\n    fn find(&self, id: u64) -> Option<User> {\n        None\n    }\n}\n\nimpl User {\n    pub fn new(name: String) -> Self {\n        Self { id: 0, name }\n    }\n}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "impl Repository for SqlUserRepository"));
        assert!(symbols.iter().any(|s| s.name == "impl User"));
    }

    #[test]
    fn test_parse_functions() {
        let content = "pub fn process_data(data: &[u8]) -> Result<(), Error> {\n    Ok(())\n}\n\nfn private_helper() {}\n\npub async fn fetch_user(id: u64) -> User {\n    todo!()\n}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "process_data"));
        assert!(symbols.iter().any(|s| s.name == "private_helper"));
        assert!(symbols.iter().any(|s| s.name == "fetch_user"));
    }

    #[test]
    fn test_parse_macro() {
        let content = "macro_rules! vec_of_strings {\n    ($($x:expr),*) => (vec![$($x.to_string()),*]);\n}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "vec_of_strings!"));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = "pub type UserId = u64;\ntype Result<T> = std::result::Result<T, Error>;\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserId" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Result" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_const_static() {
        let content = "pub const MAX_SIZE: usize = 1024;\nstatic GLOBAL_COUNTER: AtomicUsize = AtomicUsize::new(0);\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_SIZE" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "GLOBAL_COUNTER" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_modules() {
        let content = "mod tests;\npub mod utils;\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "tests" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "utils" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_derive() {
        let content =
            "#[derive(Debug, Clone, Serialize)]\npub struct Config {\n    pub name: String,\n}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "#[derive(Debug)]"));
        assert!(symbols.iter().any(|s| s.name == "#[derive(Clone)]"));
        assert!(symbols.iter().any(|s| s.name == "#[derive(Serialize)]"));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// struct FakeStruct {}\nstruct RealStruct {}\n/* fn fake_func() {} */\nfn real_func() {}\n";
        let symbols = RUST_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealStruct"));
        assert!(!symbols.iter().any(|s| s.name == "FakeStruct"));
        assert!(symbols.iter().any(|s| s.name == "real_func"));
        assert!(!symbols.iter().any(|s| s.name == "fake_func"));
    }
}
