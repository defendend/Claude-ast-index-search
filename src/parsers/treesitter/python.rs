//! Tree-sitter based Python parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static PY_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_python::LANGUAGE.into());

static PY_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&PY_LANGUAGE, include_str!("queries/python.scm"))
        .expect("Failed to compile Python tree-sitter query")
});

pub static PYTHON_PARSER: PythonParser = PythonParser;

pub struct PythonParser;

impl LanguageParser for PythonParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &PY_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*PY_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_import_name = idx("import_name");
        let idx_import_alias_original = idx("import_alias_original");
        let idx_import_alias_name = idx("import_alias_name");
        let idx_import_from_module = idx("import_from_module");
        let idx_import_from_name = idx("import_from_name");
        let idx_import_from_module_alias = idx("import_from_module_alias");
        let idx_import_from_aliased_name = idx("import_from_aliased_name");
        let idx_class_name = idx("class_name");
        let idx_class_parents = idx("class_parents");
        let idx_decorator = idx("decorator");
        let idx_func_decorator = idx("func_decorator");
        let idx_func_name = idx("func_name");
        let idx_decorated_func_name = idx("decorated_func_name");
        let idx_method_name = idx("method_name");
        let idx_decorated_method_name = idx("decorated_method_name");
        let idx_assignment_name = idx("assignment_name");
        let idx_assignment_value = idx("assignment_value");

        let mut emitted_classes = std::collections::HashSet::new();
        let mut emitted_funcs = std::collections::HashSet::new();

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Import: import X
            if let Some(cap) = find_capture(m, idx_import_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Import: import X as Y
            if let Some(orig_cap) = find_capture(m, idx_import_alias_original) {
                let original = node_text(content, &orig_cap.node);
                let line = node_line(&orig_cap.node);
                let sig = line_text(content, line).trim().to_string();

                symbols.push(ParsedSymbol {
                    name: original.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: sig.clone(),
                    parents: vec![],
                });

                if let Some(alias_cap) = find_capture(m, idx_import_alias_name) {
                    let alias = node_text(content, &alias_cap.node);
                    symbols.push(ParsedSymbol {
                        name: alias.to_string(),
                        kind: SymbolKind::Import,
                        line,
                        signature: sig,
                        parents: vec![],
                    });
                }
                continue;
            }

            // Import: from X import Y
            if let Some(mod_cap) = find_capture(m, idx_import_from_module) {
                let module = node_text(content, &mod_cap.node);
                let line = node_line(&mod_cap.node);
                let sig = line_text(content, line).trim().to_string();

                symbols.push(ParsedSymbol {
                    name: module.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: sig.clone(),
                    parents: vec![],
                });

                for cap in m
                    .captures
                    .iter()
                    .filter(|c| Some(c.index) == idx_import_from_name)
                {
                    let item = node_text(content, &cap.node);
                    if item != "*" {
                        symbols.push(ParsedSymbol {
                            name: item.to_string(),
                            kind: SymbolKind::Import,
                            line,
                            signature: sig.clone(),
                            parents: vec![],
                        });
                    }
                }
                continue;
            }

            // Import: from X import Y as Z
            if let Some(mod_cap) = find_capture(m, idx_import_from_module_alias) {
                let module = node_text(content, &mod_cap.node);
                let line = node_line(&mod_cap.node);
                let sig = line_text(content, line).trim().to_string();

                symbols.push(ParsedSymbol {
                    name: module.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: sig.clone(),
                    parents: vec![],
                });

                if let Some(name_cap) = find_capture(m, idx_import_from_aliased_name) {
                    let item = node_text(content, &name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: item.to_string(),
                        kind: SymbolKind::Import,
                        line,
                        signature: sig,
                        parents: vec![],
                    });
                }
                continue;
            }

            // Class definition (with or without parents)
            if let Some(cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                if emitted_classes.insert(line) {
                    let parents = find_capture(m, idx_class_parents)
                        .map(|pc| parse_python_parents(content, &pc.node))
                        .unwrap_or_default();
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Class,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents,
                    });
                }
                continue;
            }

            // Decorator for class
            if let Some(cap) = find_capture(m, idx_decorator) {
                let dec_text = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let name = dec_text.trim_start_matches('@');
                if is_significant_decorator(name) {
                    symbols.push(ParsedSymbol {
                        name: format!("@{}", name),
                        kind: SymbolKind::Annotation,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // Decorator for function
            if let Some(cap) = find_capture(m, idx_func_decorator) {
                let dec_text = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let name = dec_text.trim_start_matches('@');
                let name = name.split('(').next().unwrap_or(name);
                if is_significant_decorator(name) {
                    symbols.push(ParsedSymbol {
                        name: format!("@{}", name),
                        kind: SymbolKind::Annotation,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // Decorated function at module level
            if let Some(cap) = find_capture(m, idx_decorated_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                if emitted_funcs.insert(line) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // Function at module level
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                if emitted_funcs.insert(line) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // Method inside class
            if let Some(cap) = find_capture(m, idx_method_name) {
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

            // Decorated method inside class
            if let Some(cap) = find_capture(m, idx_decorated_method_name) {
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

            // Module-level assignments
            if let Some(name_cap) = find_capture(m, idx_assignment_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                let sig = line_text(content, line).trim().to_string();

                if let Some(val_cap) = find_capture(m, idx_assignment_value) {
                    let val = node_text(content, &val_cap.node);
                    if is_type_alias_value(val)
                        && name
                            .chars()
                            .next()
                            .map(|c| c.is_uppercase())
                            .unwrap_or(false)
                    {
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind: SymbolKind::TypeAlias,
                            line,
                            signature: sig,
                            parents: vec![],
                        });
                        continue;
                    }
                }

                if is_constant_name(name) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        line,
                        signature: sig,
                        parents: vec![],
                    });
                }
                continue;
            }
        }

        Ok(symbols)
    }
}

fn parse_python_parents(content: &str, node: &tree_sitter::Node) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = node_text(content, &child);
            if name != "object" {
                parents.push((name.to_string(), "extends".to_string()));
            }
        } else if child.kind() == "attribute" {
            let name = node_text(content, &child);
            if name != "object" {
                parents.push((name.to_string(), "extends".to_string()));
            }
        }
    }
    parents
}

fn is_constant_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_type_alias_value(val: &str) -> bool {
    val.starts_with("Union")
        || val.starts_with("Optional")
        || val.starts_with("List")
        || val.starts_with("Dict")
        || val.starts_with("Tuple")
        || val.starts_with("Callable")
        || val.starts_with("Type")
}

fn is_significant_decorator(name: &str) -> bool {
    name.contains("route")
        || name.contains("handler")
        || name.contains("pytest")
        || name.contains("fixture")
        || name.contains("dataclass")
        || name.contains("property")
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
    fn test_parse_class() {
        let content = "class MyClass:\n    pass\n\nclass ChildClass(ParentClass):\n    pass\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ChildClass" && s.parents.iter().any(|(p, _)| p == "ParentClass")));
    }

    #[test]
    fn test_parse_functions() {
        let content = "def handle(request, context):\n    pass\n\nasync def async_handler(request):\n    pass\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "handle" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "async_handler" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_private_functions() {
        let content = r#"
def _module_helper():
    pass

@pytest.fixture
def _client():
    return Client()

class Worker:
    def _run(self):
        pass

    @property
    def _value(self):
        return 1
"#;
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        for name in ["_module_helper", "_client", "_run", "_value"] {
            assert!(
                symbols
                    .iter()
                    .any(|s| s.name == name && s.kind == SymbolKind::Function),
                "expected private Python function {name} to be indexed"
            );
        }
    }

    #[test]
    fn test_parse_imports() {
        let content = "import logging\nfrom driver_referrals.common import db\nfrom typing import Optional, List\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "logging" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "driver_referrals.common" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_import_alias() {
        let content = "import sqlalchemy as sa\nimport numpy as np\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "sqlalchemy" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "sa" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "numpy" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "np" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_decorators() {
        let content = "@dataclass\nclass Config:\n    host: str\n\n@property\ndef name(self):\n    return self._name\n\n@pytest.fixture\ndef client():\n    return Client()\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "@dataclass"));
        assert!(symbols.iter().any(|s| s.name == "@property"));
        assert!(symbols.iter().any(|s| s.name == "@pytest.fixture"));
    }

    #[test]
    fn test_parse_constants() {
        let content = "MAX_RETRIES = 5\nDEFAULT_TIMEOUT = 30\nAPI_KEY = \"secret\"\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "DEFAULT_TIMEOUT" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "API_KEY" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_type_aliases() {
        let content = "UserList = List[User]\nCallback = Callable[[str], None]\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserList" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Callback" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_class_multiple_inheritance() {
        let content = "class MyView(BaseView, PermissionMixin, LoggingMixin):\n    pass\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "MyView").unwrap();
        assert!(cls.parents.iter().any(|(p, _)| p == "BaseView"));
        assert!(cls.parents.iter().any(|(p, _)| p == "PermissionMixin"));
        assert!(cls.parents.iter().any(|(p, _)| p == "LoggingMixin"));
    }

    #[test]
    fn test_parse_function_with_return_type() {
        let content = "def get_name(self) -> str:\n    return \"\"\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "get_name"));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "# class FakeClass:\n#     pass\nclass RealClass:\n    pass\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
    }

    #[test]
    fn test_async_functions() {
        let content = "async def fetch_data(url) -> str:\n    pass\n\nasync def process_event(event):\n    pass\n";
        let symbols = PYTHON_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "fetch_data" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "process_event" && s.kind == SymbolKind::Function));
    }
}
