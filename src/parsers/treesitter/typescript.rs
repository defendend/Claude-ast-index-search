//! Tree-sitter based TypeScript/JavaScript parser

use anyhow::Result;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};
use std::sync::LazyLock;

use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;
use super::{LanguageParser, parse_tree, node_text, node_line, line_text};

static TS_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_typescript::LANGUAGE_TSX.into());

static TS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&TS_LANGUAGE, include_str!("queries/typescript.scm"))
        .expect("Failed to compile TypeScript tree-sitter query")
});

pub static TYPESCRIPT_PARSER: TypeScriptParser = TypeScriptParser;

pub struct TypeScriptParser;

/// Significant decorators to track
const SIGNIFICANT_DECORATORS: &[&str] = &[
    "Controller", "Get", "Post", "Put", "Delete", "Patch",
    "Injectable", "Module", "Component", "Service", "Entity", "Column",
];

/// Check if a name is PascalCase (starts with uppercase letter)
fn is_pascal_case(name: &str) -> bool {
    name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Check if a name is a React hook (starts with "use" followed by uppercase)
fn is_hook(name: &str) -> bool {
    name.starts_with("use")
        && name.len() > 3
        && name.chars().nth(3).map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Check if a name is ALL_CAPS constant
fn is_all_caps(name: &str) -> bool {
    !name.is_empty()
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        && name.chars().all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Check if an import source is a relative/local import
fn is_relative_import(source: &str) -> bool {
    source.starts_with('.')
        || source.starts_with("@/")
        || source.starts_with('~')
}

/// Extract parent types from a class_heritage node (extends_clause, implements_clause)
fn extract_class_parents(content: &str, class_node: &tree_sitter::Node) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = class_node.walk();

    for child in class_node.children(&mut cursor) {
        if child.kind() == "class_heritage" {
            let mut heritage_cursor = child.walk();
            for heritage_child in child.children(&mut heritage_cursor) {
                if heritage_child.kind() == "extends_clause" {
                    // extends_clause has a "value" field
                    let mut ec_cursor = heritage_child.walk();
                    for ec_child in heritage_child.children(&mut ec_cursor) {
                        match ec_child.kind() {
                            "identifier" | "type_identifier" | "nested_identifier" => {
                                let name = node_text(content, &ec_child);
                                // Strip generic type arguments if present
                                let name = name.split('<').next().unwrap_or(name).trim();
                                if !name.is_empty() {
                                    parents.push((name.to_string(), "extends".to_string()));
                                }
                            }
                            "generic_type" => {
                                // Generic type like BaseService<T> - get the first named child (type name)
                                if let Some(first) = ec_child.named_child(0) {
                                    let kind = first.kind();
                                    if kind == "type_identifier" || kind == "identifier" || kind == "nested_identifier" {
                                        let name = node_text(content, &first);
                                        parents.push((name.to_string(), "extends".to_string()));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                } else if heritage_child.kind() == "implements_clause" {
                    let mut ic_cursor = heritage_child.walk();
                    for ic_child in heritage_child.children(&mut ic_cursor) {
                        match ic_child.kind() {
                            "type_identifier" | "identifier" | "nested_identifier" => {
                                let name = node_text(content, &ic_child);
                                let name = name.split('<').next().unwrap_or(name).trim();
                                if !name.is_empty() {
                                    parents.push((name.to_string(), "implements".to_string()));
                                }
                            }
                            "generic_type" => {
                                if let Some(first) = ic_child.named_child(0) {
                                    let kind = first.kind();
                                    if kind == "type_identifier" || kind == "identifier" || kind == "nested_identifier" {
                                        let name = node_text(content, &first);
                                        parents.push((name.to_string(), "implements".to_string()));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    parents
}

/// Extract parent types from an interface's extends_type_clause
fn extract_interface_parents(content: &str, iface_node: &tree_sitter::Node) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = iface_node.walk();

    for child in iface_node.children(&mut cursor) {
        if child.kind() == "extends_type_clause" {
            let mut etc_cursor = child.walk();
            for etc_child in child.children(&mut etc_cursor) {
                match etc_child.kind() {
                    "type_identifier" | "identifier" | "nested_identifier" | "nested_type_identifier" => {
                        let name = node_text(content, &etc_child);
                        let name = name.split('<').next().unwrap_or(name).trim();
                        if !name.is_empty() {
                            parents.push((name.to_string(), "extends".to_string()));
                        }
                    }
                    "generic_type" => {
                        if let Some(first) = etc_child.named_child(0) {
                            let kind = first.kind();
                            if kind == "type_identifier" || kind == "identifier" || kind == "nested_type_identifier" {
                                let name = node_text(content, &first);
                                parents.push((name.to_string(), "extends".to_string()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    parents
}

impl LanguageParser for TypeScriptParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &TS_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*TS_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names.iter().position(|n| *n == name).map(|i| i as u32)
        };

        // Class captures
        let idx_class_name = idx("class_name");
        let idx_class_node = idx("class_node");
        let idx_abstract_class_name = idx("abstract_class_name");
        let idx_abstract_class_node = idx("abstract_class_node");
        let idx_export_class_name = idx("export_class_name");
        let idx_export_class_node = idx("export_class_node");
        let idx_export_abstract_class_name = idx("export_abstract_class_name");
        let idx_export_abstract_class_node = idx("export_abstract_class_node");

        // Interface captures
        let idx_interface_name = idx("interface_name");
        let idx_interface_node = idx("interface_node");
        let idx_export_interface_name = idx("export_interface_name");
        let idx_export_interface_node = idx("export_interface_node");

        // Type alias captures
        let idx_type_alias_name = idx("type_alias_name");
        let idx_export_type_alias_name = idx("export_type_alias_name");

        // Enum captures
        let idx_enum_name = idx("enum_name");
        let idx_export_enum_name = idx("export_enum_name");

        // Function captures
        let idx_func_name = idx("func_name");
        let idx_export_func_name = idx("export_func_name");

        // Arrow function captures
        let idx_arrow_func_name = idx("arrow_func_name");
        let idx_export_arrow_func_name = idx("export_arrow_func_name");

        // Constant captures
        let idx_const_name = idx("const_name");
        let idx_export_const_name = idx("export_const_name");

        // Namespace captures
        let idx_namespace_name = idx("namespace_name");
        let idx_export_namespace_name = idx("export_namespace_name");

        // Import captures
        let idx_import_source = idx("import_source");

        // Decorator captures
        let idx_decorator_id = idx("decorator_id");
        let idx_decorator_call_id = idx("decorator_call_id");

        // Method captures
        let idx_method_name = idx("method_name");
        let idx_method_node = idx("method_node");
        let idx_private_method_name = idx("private_method_name");
        let idx_private_method_node = idx("private_method_node");

        // Field captures
        let idx_field_name = idx("field_name");
        let idx_field_node = idx("field_node");
        let idx_private_field_name = idx("private_field_name");
        let idx_private_field_node = idx("private_field_node");

        // Abstract method captures
        let idx_abstract_method_name = idx("abstract_method_name");
        let idx_abstract_method_node = idx("abstract_method_node");

        // Track emitted symbols to avoid duplicates
        let mut emitted_lines: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // === Classes ===

            // class Name (non-exported)
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let parents = find_capture(m, idx_class_node)
                        .map(|n| extract_class_parents(content, &n.node))
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

            // abstract class Name (non-exported)
            if let Some(name_cap) = find_capture(m, idx_abstract_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let parents = find_capture(m, idx_abstract_class_node)
                        .map(|n| extract_class_parents(content, &n.node))
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

            // export class Name
            if let Some(name_cap) = find_capture(m, idx_export_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let parents = find_capture(m, idx_export_class_node)
                        .map(|n| extract_class_parents(content, &n.node))
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

            // export abstract class Name
            if let Some(name_cap) = find_capture(m, idx_export_abstract_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let parents = find_capture(m, idx_export_abstract_class_node)
                        .map(|n| extract_class_parents(content, &n.node))
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

            // === Interfaces ===

            if let Some(name_cap) = find_capture(m, idx_interface_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let parents = find_capture(m, idx_interface_node)
                        .map(|n| extract_interface_parents(content, &n.node))
                        .unwrap_or_default();
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Interface,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents,
                    });
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_interface_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let parents = find_capture(m, idx_export_interface_node)
                        .map(|n| extract_interface_parents(content, &n.node))
                        .unwrap_or_default();
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Interface,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents,
                    });
                }
                continue;
            }

            // === Type aliases ===

            if let Some(name_cap) = find_capture(m, idx_type_alias_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::TypeAlias,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_type_alias_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::TypeAlias,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Enums ===

            if let Some(name_cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Enum,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_enum_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Enum,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Functions ===
            // function name() { } - classify by name pattern

            if let Some(name_cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let kind = classify_function_name(name);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_func_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let kind = classify_function_name(name);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Arrow functions ===
            // const name = (...) => { }

            if let Some(name_cap) = find_capture(m, idx_arrow_func_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let kind = classify_function_name(name);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_arrow_func_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    let kind = classify_function_name(name);
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Constants (ALL_CAPS) ===
            // These patterns also match arrow functions and other variables,
            // so we only emit if it looks like ALL_CAPS and wasn't already emitted.

            if let Some(name_cap) = find_capture(m, idx_const_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if is_all_caps(name) && emitted_lines.insert((name.to_string(), line)) {
                    // Check that this is at module level (parent chain: variable_declarator -> lexical_declaration -> program)
                    let decl_node = name_cap.node.parent(); // variable_declarator
                    let lex_node = decl_node.and_then(|n| n.parent()); // lexical_declaration
                    let parent_node = lex_node.and_then(|n| n.parent()); // should be program
                    let is_module_level = parent_node
                        .map(|n| n.kind() == "program")
                        .unwrap_or(false);

                    if is_module_level {
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind: SymbolKind::Constant,
                            line,
                            signature: line_text(content, line).trim().to_string(),
                            parents: vec![],
                        });
                    }
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_const_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if is_all_caps(name) && emitted_lines.insert((name.to_string(), line)) {
                    // Export statement is always module-level
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Namespaces ===

            if let Some(name_cap) = find_capture(m, idx_namespace_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Package,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_namespace_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Package,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Imports ===

            if let Some(source_cap) = find_capture(m, idx_import_source) {
                let raw_source = node_text(content, &source_cap.node);
                let line = node_line(&source_cap.node);
                // Strip quotes from source
                let source = raw_source.trim_matches(|c| c == '\'' || c == '"');
                if is_relative_import(source) {
                    symbols.push(ParsedSymbol {
                        name: source.to_string(),
                        kind: SymbolKind::Import,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
                continue;
            }

            // === Decorators ===

            if let Some(dec_cap) = find_capture(m, idx_decorator_id) {
                let name = node_text(content, &dec_cap.node);
                let line = node_line(&dec_cap.node);
                if SIGNIFICANT_DECORATORS.iter().any(|s| name.contains(s)) {
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

            if let Some(dec_cap) = find_capture(m, idx_decorator_call_id) {
                let name = node_text(content, &dec_cap.node);
                let line = node_line(&dec_cap.node);
                if SIGNIFICANT_DECORATORS.iter().any(|s| name.contains(s)) {
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

            // === Class methods ===

            if emit_class_member(content, m, idx_method_name, idx_method_node, SymbolKind::Function, &mut symbols, &mut emitted_lines) {
                continue;
            }
            if emit_class_member(content, m, idx_private_method_name, idx_private_method_node, SymbolKind::Function, &mut symbols, &mut emitted_lines) {
                continue;
            }

            // === Class fields/properties ===

            if emit_class_member(content, m, idx_field_name, idx_field_node, SymbolKind::Property, &mut symbols, &mut emitted_lines) {
                continue;
            }
            if emit_class_member(content, m, idx_private_field_name, idx_private_field_node, SymbolKind::Property, &mut symbols, &mut emitted_lines) {
                continue;
            }

            // === Abstract methods ===

            if emit_class_member(content, m, idx_abstract_method_name, idx_abstract_method_node, SymbolKind::Function, &mut symbols, &mut emitted_lines) {
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Check if a node is inside a class_body (class member, not object literal method)
fn is_inside_class_body(node: &tree_sitter::Node) -> bool {
    node.parent()
        .map(|p| p.kind() == "class_body")
        .unwrap_or(false)
}

/// Emit a class member symbol (method or field) if it's inside a class body
fn emit_class_member(
    content: &str,
    m: &tree_sitter::QueryMatch,
    idx_name: Option<u32>,
    idx_node: Option<u32>,
    kind: SymbolKind,
    symbols: &mut Vec<ParsedSymbol>,
    emitted_lines: &mut std::collections::HashSet<(String, usize)>,
) -> bool {
    if let Some(name_cap) = find_capture(m, idx_name) {
        if let Some(node_cap) = find_capture(m, idx_node) {
            if is_inside_class_body(&node_cap.node) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                    });
                }
            }
        }
        return true;
    }
    false
}

/// Classify a function/arrow-function name into the appropriate SymbolKind:
/// - PascalCase -> Class (React component)
/// - useXxx -> Function (React hook)
/// - lowercase -> Function
fn classify_function_name(name: &str) -> SymbolKind {
    if is_hook(name) {
        SymbolKind::Function
    } else if is_pascal_case(name) {
        SymbolKind::Class // React component
    } else {
        SymbolKind::Function
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
    fn test_parse_class() {
        let content = "export class UserService extends BaseService implements IUserService {\n}\n\nclass ChildClass extends ParentClass {\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "ChildClass" && s.parents.iter().any(|(p, _)| p == "ParentClass")));
    }

    #[test]
    fn test_parse_interface() {
        let content = "interface User {\n    id: string;\n}\n\nexport interface IUserService extends IService {\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "IUserService" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = "type UserId = string;\nexport type UserMap = Map<string, User>;\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserId" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "UserMap" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_enum() {
        let content = "enum Status {\n    Active,\n    Inactive,\n}\n\nexport const enum Direction {\n    Up,\n    Down,\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols.iter().any(|s| s.name == "Direction" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_functions() {
        let content = "function handleRequest(req: Request): Response {\n    return new Response();\n}\n\nexport async function fetchUser(id: string): Promise<User> {\n    return fetch(`/users/${id}`);\n}\n\nconst processData = (data: Data) => {\n    return data;\n};\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "handleRequest" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "fetchUser" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "processData" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_react_component() {
        let content = "const Button: React.FC<ButtonProps> = ({ children, onClick }) => {\n    return <button onClick={onClick}>{children}</button>;\n};\n\nexport function UserCard({ user }: UserCardProps) {\n    return <div>{user.name}</div>;\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Button" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "UserCard" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_react_hooks() {
        let content = "function useAuth() {\n    const [user, setUser] = useState(null);\n    return { user };\n}\n\nexport const useCounter = () => {\n    return { count: 0 };\n};\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "useAuth" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "useCounter" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_constants() {
        let content = "const API_URL = 'https://api.example.com';\nexport const MAX_RETRIES = 3;\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "API_URL" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_namespace() {
        let content = "namespace Utils {\n    export function helper() {}\n}\n\nexport namespace Types {\n    export interface User {}\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Utils" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "Types" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_decorators() {
        let content = "@Controller('users')\nexport class UserController {\n    @Get(':id')\n    getUser(@Param('id') id: string) {}\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "@Controller" && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(|s| s.name == "@Get" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// class FakeClass {}\nclass RealClass {}\n/* function fakeFunc() {} */\nfunction realFunc() {}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(symbols.iter().any(|s| s.name == "realFunc"));
        assert!(!symbols.iter().any(|s| s.name == "fakeFunc"));
    }

    #[test]
    fn test_parse_class_methods() {
        let content = r#"
export class UserService {
    constructor(private http: HttpClient) {}
    getUser(id: string): User {
        return this.http.get(id);
    }
    private validate(data: any): boolean {
        return true;
    }
}
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "constructor" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "getUser" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "validate" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_getters_setters() {
        let content = r#"
class Config {
    get value(): string { return ''; }
    set value(v: string) {}
    static create(): Config { return new Config(); }
}
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "value" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "create" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_class_fields() {
        let content = r#"
class User {
    name: string;
    readonly age: number = 0;
    static count: number = 0;
}
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "name" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "age" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "count" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_abstract_methods() {
        let content = r#"
abstract class Base {
    abstract process(data: string): void;
    abstract get name(): string;
}
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "process" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_object_literal_methods_not_indexed() {
        let content = r#"
const obj = {
    method() { return 1; },
    get prop() { return 2; },
};
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(!symbols.iter().any(|s| s.name == "method" && s.kind == SymbolKind::Function));
        assert!(!symbols.iter().any(|s| s.name == "prop" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_private_class_members() {
        let content = r#"
class Foo {
    #secret: string = '';
    #process(): void {}
}
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "#secret" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "#process" && s.kind == SymbolKind::Function));
    }
}
