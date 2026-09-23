//! Tree-sitter based Go parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static GO_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_go::LANGUAGE.into());

static GO_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&GO_LANGUAGE, include_str!("queries/go.scm"))
        .expect("Failed to compile Go tree-sitter query")
});

pub static GO_PARSER: GoParser = GoParser;

pub struct GoParser;

impl LanguageParser for GoParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &GO_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*GO_QUERY;

        // Build capture name → index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_package = idx("package");
        let idx_import_alias = idx("import_alias");
        let idx_import_path = idx("import_path");
        let idx_struct_name = idx("struct_name");
        let idx_interface_name = idx("interface_name");
        let idx_type_alias_name = idx("type_alias_name");
        let idx_type_alias_target = idx("type_alias_target");
        let idx_func_name = idx("func_name");
        let idx_method_receiver = idx("method_receiver");
        let idx_method_name = idx("method_name");
        let idx_method_receiver_value = idx("method_receiver_value");
        let idx_method_name_value = idx("method_name_value");
        let idx_const_name = idx("const_name");
        let idx_var_name = idx("var_name");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Package
            if let Some(cap) = find_capture(m, idx_package) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
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

            // Import
            if let Some(path_cap) = find_capture(m, idx_import_path) {
                let raw_path = node_text(content, &path_cap.node);
                // Strip quotes
                let path = raw_path.trim_matches('"');
                let alias = find_capture(m, idx_import_alias).map(|c| node_text(content, &c.node));
                let line = node_line(&path_cap.node);

                let name = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path));

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: if alias.is_some() {
                        format!("import {} \"{}\"", name, path)
                    } else {
                        format!("import \"{}\"", path)
                    },
                    parents: vec![(path.to_string(), "from".to_string())],
                    end_line,
                });
                continue;
            }

            // Struct
            if let Some(cap) = find_capture(m, idx_struct_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: line_text(content, line).trim().to_string(),
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
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Type alias
            if let Some(name_cap) = find_capture(m, idx_type_alias_name) {
                let target_cap = find_capture(m, idx_type_alias_target);
                if let Some(target_cap) = target_cap {
                    let name = node_text(content, &name_cap.node);
                    let target = node_text(content, &target_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::TypeAlias,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![(target.to_string(), "alias".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Method with pointer receiver
            if let Some(recv_cap) = find_capture(m, idx_method_receiver) {
                if let Some(name_cap) = find_capture(m, idx_method_name) {
                    let receiver = node_text(content, &recv_cap.node);
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![(receiver.to_string(), "receiver".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Method with value receiver
            if let Some(recv_cap) = find_capture(m, idx_method_receiver_value) {
                if let Some(name_cap) = find_capture(m, idx_method_name_value) {
                    let receiver = node_text(content, &recv_cap.node);
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![(receiver.to_string(), "receiver".to_string())],
                        end_line,
                    });
                }
                continue;
            }

            // Function
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

            // Var
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
    fn test_parse_package() {
        let content = "package main\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "main" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_struct() {
        let content = "package main\n\ntype DeleteAction struct {\n    avaSrv AvatarsMDS\n}\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "DeleteAction" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_interface() {
        let content = "package main\n\ntype AvatarsMDS interface {\n    Delete(ctx context.Context) error\n}\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "AvatarsMDS" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_method_pointer_receiver() {
        let content = "package main\n\nfunc (a *DeleteAction) Do(ctx context.Context) error {\n    return nil\n}\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Do"
            && s.kind == SymbolKind::Function
            && s.parents
                .iter()
                .any(|(p, k)| p == "DeleteAction" && k == "receiver")));
    }

    #[test]
    fn test_parse_method_value_receiver() {
        let content =
            "package main\n\nfunc (a DeleteAction) String() string {\n    return \"\"\n}\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "String"
            && s.kind == SymbolKind::Function
            && s.parents
                .iter()
                .any(|(p, k)| p == "DeleteAction" && k == "receiver")));
    }

    #[test]
    fn test_parse_function() {
        let content = "package main\n\nfunc NewDeleteAction() *DeleteAction {\n    return &DeleteAction{}\n}\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "NewDeleteAction" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_import_block() {
        let content = "package main\n\nimport (\n    \"fmt\"\n    \"net/http\"\n    log \"github.com/sirupsen/logrus\"\n)\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "fmt" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "http" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "log" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_single_import() {
        let content = "package main\n\nimport \"fmt\"\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "fmt" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_const_block() {
        let content = "package main\n\nconst (\n    StatusActive = iota\n    StatusDeleted\n    MaxRetries int = 5\n)\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "StatusActive" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MaxRetries" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_single_const() {
        let content = "package main\n\nconst SingleConst = \"hello\"\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "SingleConst" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_var() {
        let content = "package main\n\nvar PublicVar string\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "PublicVar" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = "package main\n\ntype UserID int64\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserID" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_exported_vs_unexported() {
        let content = "package main\n\ntype PublicStruct struct {}\nfunc PublicFunc() {}\nfunc privateFunc() {}\nvar PublicVar string\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "PublicStruct"));
        assert!(symbols.iter().any(|s| s.name == "PublicFunc"));
        assert!(symbols.iter().any(|s| s.name == "privateFunc"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "PublicVar" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "package main\n\n// type FakeStruct struct {}\ntype RealStruct struct {}\n/* func FakeFunc() {} */\nfunc RealFunc() {}\n";
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealStruct"));
        assert!(!symbols.iter().any(|s| s.name == "FakeStruct"));
        assert!(symbols.iter().any(|s| s.name == "RealFunc"));
        assert!(!symbols.iter().any(|s| s.name == "FakeFunc"));
    }

    #[test]
    fn test_multiline_function() {
        let content = r#"package main

func VeryLongFunction(
    arg1 string,
    arg2 int,
    arg3 *SomeType,
) error {
    return nil
}
"#;
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "VeryLongFunction" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_interface_embedding() {
        let content = r#"package main

type Reader interface {
    Read(p []byte) (n int, err error)
}

type ReadWriter interface {
    Reader
    Write(p []byte) (n int, err error)
}
"#;
        let symbols = GO_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Reader" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ReadWriter" && s.kind == SymbolKind::Interface));
    }
}
