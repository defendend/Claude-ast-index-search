//! Tree-sitter based Matlab parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static MATLAB_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_matlab::LANGUAGE.into());

static MATLAB_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&MATLAB_LANGUAGE, include_str!("queries/matlab.scm"))
        .expect("Failed to compile Matlab tree-sitter query")
});

pub static MATLAB_PARSER: MatlabParser = MatlabParser;

pub struct MatlabParser;

impl LanguageParser for MatlabParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &MATLAB_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*MATLAB_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_class_name = idx("class_name");
        let idx_func_name = idx("func_name");
        let idx_property_name = idx("property_name");
        let idx_enum_name = idx("enum_name");
        let idx_event_name = idx("event_name");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = m
                .captures
                .iter()
                .find(|c| Some(c.index) == idx_definition)
                .map(|c| text_end_line(content, &c.node));

            for cap in m.captures {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let sig = line_text(content, line).trim().to_string();

                if Some(cap.index) == idx_class_name {
                    // Extract superclasses from the class_definition node
                    let class_node = cap.node.parent().unwrap();
                    let parents = extract_superclasses(content, &class_node);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Class,
                        line,
                        signature: sig,
                        parents,
                        end_line,
                    });
                } else if Some(cap.index) == idx_func_name {
                    // Check if this function is inside a class (method) or standalone
                    let parent_class = find_parent_class(content, &cap.node);
                    let kind = SymbolKind::Function;
                    let parents = if let Some(class_name) = parent_class {
                        vec![(class_name, "member_of".to_string())]
                    } else {
                        vec![]
                    };
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: sig,
                        parents,
                        end_line,
                    });
                } else if Some(cap.index) == idx_property_name {
                    let parent_class = find_parent_class(content, &cap.node);
                    let parents = if let Some(class_name) = parent_class {
                        vec![(class_name, "member_of".to_string())]
                    } else {
                        vec![]
                    };
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Property,
                        line,
                        signature: sig,
                        parents,
                        end_line,
                    });
                } else if Some(cap.index) == idx_enum_name {
                    let parent_class = find_parent_class(content, &cap.node);
                    let parents = if let Some(class_name) = parent_class {
                        vec![(class_name, "member_of".to_string())]
                    } else {
                        vec![]
                    };
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        line,
                        signature: sig,
                        parents,
                        end_line,
                    });
                } else if Some(cap.index) == idx_event_name {
                    let parent_class = find_parent_class(content, &cap.node);
                    let parents = if let Some(class_name) = parent_class {
                        vec![(class_name, "member_of".to_string())]
                    } else {
                        vec![]
                    };
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Property,
                        line,
                        signature: sig,
                        parents,
                        end_line,
                    });
                }
            }
        }

        Ok(symbols)
    }
}

/// Walk up the tree to find a parent class_definition and return its name
fn find_parent_class(content: &str, node: &tree_sitter::Node) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == "class_definition" {
            // Get the name child
            let mut cursor = n.walk();
            for child in n.children(&mut cursor) {
                if child.kind() == "identifier" {
                    return Some(node_text(content, &child).to_string());
                }
            }
        }
        current = n.parent();
    }
    None
}

/// Extract superclass names from a class_definition node
fn extract_superclasses(content: &str, class_node: &tree_sitter::Node) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = class_node.walk();
    for child in class_node.children(&mut cursor) {
        if child.kind() == "superclasses" {
            let mut sc_cursor = child.walk();
            for sc_child in child.children(&mut sc_cursor) {
                if sc_child.kind() == "identifier" || sc_child.kind() == "property_name" {
                    let name = node_text(content, &sc_child);
                    parents.push((name.to_string(), "extends".to_string()));
                }
            }
        }
    }
    parents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_function() {
        let content = "function result = myFunction(x, y)\n    result = x + y;\nend\n";
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "myFunction" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_class() {
        let content = r#"classdef MyClass
    properties
        Value
    end
    methods
        function obj = MyClass(val)
            obj.Value = val;
        end
    end
end
"#;
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Value" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_class_with_superclass() {
        let content = r#"classdef Vehicle < handle
    properties
        Make
        Model
    end
    methods
        function obj = Vehicle(make, model)
            obj.Make = make;
            obj.Model = model;
        end
    end
end
"#;
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.name == "Vehicle" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(class
            .parents
            .iter()
            .any(|(name, kind)| name == "handle" && kind == "extends"));
    }

    #[test]
    fn test_parse_enumeration() {
        let content = r#"classdef Color
    enumeration
        Red, Green, Blue
    end
end
"#;
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Red" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Green" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Blue" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_events() {
        let content = r#"classdef Button < handle
    events
        ButtonPressed
        ButtonReleased
    end
end
"#;
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "ButtonPressed" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ButtonReleased" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_method_parent_class() {
        let content = r#"classdef Calculator
    methods
        function result = add(obj, a, b)
            result = a + b;
        end
    end
end
"#;
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        let method = symbols
            .iter()
            .find(|s| s.name == "add" && s.kind == SymbolKind::Function)
            .unwrap();
        assert!(method
            .parents
            .iter()
            .any(|(name, kind)| name == "Calculator" && kind == "member_of"));
    }

    #[test]
    fn test_standalone_function() {
        let content = "function y = helper(x)\n    y = x * 2;\nend\n";
        let symbols = MATLAB_PARSER.parse_symbols(content).unwrap();
        let func = symbols
            .iter()
            .find(|s| s.name == "helper" && s.kind == SymbolKind::Function)
            .unwrap();
        assert!(func.parents.is_empty());
    }
}
