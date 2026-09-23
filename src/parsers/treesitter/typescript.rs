//! Tree-sitter based TypeScript/JavaScript parser

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_end_line, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::{truncate_context, FileType, ParsedRef, ParsedSymbol};

static TS_LANGUAGE: LazyLock<Language> =
    LazyLock::new(|| tree_sitter_typescript::LANGUAGE_TSX.into());

static TS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&TS_LANGUAGE, include_str!("queries/typescript.scm"))
        .expect("Failed to compile TypeScript tree-sitter query")
});

pub static TYPESCRIPT_PARSER: TypeScriptParser = TypeScriptParser;

pub struct TypeScriptParser;

type ScopeChain = Vec<(usize, usize)>;

#[derive(Debug, Clone)]
struct AliasBinding {
    declared_line: usize,
    scope_chain: ScopeChain,
    target: String,
}

/// Significant decorators to track
const SIGNIFICANT_DECORATORS: &[&str] = &[
    "Controller",
    "Get",
    "Post",
    "Put",
    "Delete",
    "Patch",
    "Injectable",
    "Module",
    "Component",
    "Service",
    "Entity",
    "Column",
];

/// Check if a name is PascalCase (starts with uppercase letter)
fn is_pascal_case(name: &str) -> bool {
    name.chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
}

/// Check if a name is a React hook (starts with "use" followed by uppercase)
fn is_hook(name: &str) -> bool {
    name.starts_with("use")
        && name.len() > 3
        && name
            .chars()
            .nth(3)
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
}

/// Check if a name is ALL_CAPS constant
fn is_all_caps(name: &str) -> bool {
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

/// Vue/Pinia Composition API functions that create reactive state
const REACTIVE_CALL_NAMES: &[&str] = &[
    "ref",
    "reactive",
    "computed",
    "readonly",
    "shallowRef",
    "shallowReactive",
    "shallowReadonly",
    "toRef",
    "toRefs",
    "customRef",
];

/// Vue macros commonly used at module level
const DEFINE_MACRO_NAMES: &[&str] = &[
    "defineProps",
    "defineEmits",
    "defineModel",
    "defineStore",
    "defineExpose",
    "withDefaults",
];

/// Check if a tree-sitter node is a call to a Vue Composition API reactive function
/// (e.g. `ref(0)`, `computed(() => ...)`, `defineStore('id', () => ...)`)
fn is_composition_api_call<'a>(
    content: &str,
    name_node: &tree_sitter::Node<'a>,
) -> Option<&'static str> {
    // name_node is the identifier (e.g. "count")
    // parent is variable_declarator: name = value
    let var_decl = name_node.parent()?;
    if var_decl.kind() != "variable_declarator" {
        return None;
    }

    // Get the value node (right side of =)
    let value_node = var_decl.child_by_field_name("value")?;

    // The value might be a call_expression directly, or wrapped in `as` expression
    let call_node = if value_node.kind() == "call_expression" {
        value_node
    } else if value_node.kind() == "as_expression" {
        // const x = ref(0) as Ref<number>
        let inner = value_node.named_child(0)?;
        if inner.kind() == "call_expression" {
            inner
        } else {
            return None;
        }
    } else {
        return None;
    };

    // Get the function being called
    let func_node = call_node.child_by_field_name("function")?;
    let func_name = node_text(content, &func_node);

    if REACTIVE_CALL_NAMES.contains(&func_name) || DEFINE_MACRO_NAMES.contains(&func_name) {
        Some(if REACTIVE_CALL_NAMES.contains(&func_name) {
            "reactive"
        } else {
            "macro"
        })
    } else {
        None
    }
}

/// Check if an import source is a relative/local import
fn is_relative_import(source: &str) -> bool {
    source.starts_with('.') || source.starts_with("@/") || source.starts_with('~')
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
                                    if kind == "type_identifier"
                                        || kind == "identifier"
                                        || kind == "nested_identifier"
                                    {
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
                                    if kind == "type_identifier"
                                        || kind == "identifier"
                                        || kind == "nested_identifier"
                                    {
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
fn extract_interface_parents(
    content: &str,
    iface_node: &tree_sitter::Node,
) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = iface_node.walk();

    for child in iface_node.children(&mut cursor) {
        if child.kind() == "extends_type_clause" {
            let mut etc_cursor = child.walk();
            for etc_child in child.children(&mut etc_cursor) {
                match etc_child.kind() {
                    "type_identifier"
                    | "identifier"
                    | "nested_identifier"
                    | "nested_type_identifier" => {
                        let name = node_text(content, &etc_child);
                        let name = name.split('<').next().unwrap_or(name).trim();
                        if !name.is_empty() {
                            parents.push((name.to_string(), "extends".to_string()));
                        }
                    }
                    "generic_type" => {
                        if let Some(first) = etc_child.named_child(0) {
                            let kind = first.kind();
                            if kind == "type_identifier"
                                || kind == "identifier"
                                || kind == "nested_type_identifier"
                            {
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
    fn extract_refs(&self, content: &str, defined: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
        self.typescript_extract_refs(content, defined, None)
    }

    fn extract_refs_for_lang(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        file_type: FileType,
    ) -> Result<Vec<ParsedRef>> {
        self.typescript_extract_refs(content, defined, Some(file_type))
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &TS_LANGUAGE)?;
        let mut symbols = Vec::new();
        let query = &*TS_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
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
        let idx_type_alias_node = idx("type_alias_node");
        let idx_export_type_alias_name = idx("export_type_alias_name");
        let idx_export_type_alias_node = idx("export_type_alias_node");

        // Enum captures
        let idx_enum_name = idx("enum_name");
        let idx_enum_node = idx("enum_node");
        let idx_export_enum_name = idx("export_enum_name");
        let idx_export_enum_node = idx("export_enum_node");

        // Function captures
        let idx_func_name = idx("func_name");
        let idx_func_node = idx("func_node");
        let idx_export_func_name = idx("export_func_name");
        let idx_export_func_node = idx("export_func_node");

        // Arrow function captures
        let idx_arrow_func_name = idx("arrow_func_name");
        let idx_arrow_func_node = idx("arrow_func_node");
        let idx_export_arrow_func_name = idx("export_arrow_func_name");
        let idx_export_arrow_func_node = idx("export_arrow_func_node");

        // Constant captures
        let idx_const_name = idx("const_name");
        let idx_const_node = idx("const_node");
        let idx_export_const_name = idx("export_const_name");
        let idx_export_const_node = idx("export_const_node");

        // Namespace captures
        let idx_namespace_name = idx("namespace_name");
        let idx_namespace_node = idx("namespace_node");
        let idx_export_namespace_name = idx("export_namespace_name");
        let idx_export_namespace_node = idx("export_namespace_node");

        // Ambient const captures (declare const without value)
        let idx_export_ambient_const_name = idx("export_ambient_const_name");
        let idx_export_ambient_const_node = idx("export_ambient_const_node");

        // Export default captures
        let idx_export_default_value = idx("export_default_value");

        // Import captures
        let idx_import_source = idx("import_source");
        let idx_import_node = idx("import_node");

        // Decorator captures
        let idx_decorator_id = idx("decorator_id");
        let idx_decorator_node = idx("decorator_node");
        let idx_decorator_call_id = idx("decorator_call_id");
        let idx_decorator_call_node = idx("decorator_call_node");

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

        let end_line_of = |m: &tree_sitter::QueryMatch, capture: Option<u32>| {
            find_capture(m, capture).map(|c| node_end_line(&c.node))
        };

        // Track emitted symbols to avoid duplicates
        let mut emitted_lines: std::collections::HashSet<(String, usize)> =
            std::collections::HashSet::new();

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
                        end_line: end_line_of(m, idx_class_node),
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
                        end_line: end_line_of(m, idx_abstract_class_node),
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
                        end_line: end_line_of(m, idx_export_class_node),
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
                        end_line: end_line_of(m, idx_export_abstract_class_node),
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
                        end_line: end_line_of(m, idx_interface_node),
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
                        end_line: end_line_of(m, idx_export_interface_node),
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
                        end_line: end_line_of(m, idx_type_alias_node),
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
                        end_line: end_line_of(m, idx_export_type_alias_node),
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
                        end_line: end_line_of(m, idx_enum_node),
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
                        end_line: end_line_of(m, idx_export_enum_node),
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
                        end_line: end_line_of(m, idx_func_node),
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
                        end_line: end_line_of(m, idx_export_func_node),
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
                        end_line: end_line_of(m, idx_arrow_func_node),
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
                        end_line: end_line_of(m, idx_export_arrow_func_node),
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
                if emitted_lines.insert((name.to_string(), line)) {
                    // Check for Vue Composition API calls: const x = ref(), computed(), etc.
                    if let Some(api_kind) = is_composition_api_call(content, &name_cap.node) {
                        let kind = if api_kind == "macro" {
                            SymbolKind::Function
                        } else {
                            SymbolKind::Property
                        };
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind,
                            line,
                            signature: line_text(content, line).trim().to_string(),
                            parents: vec![],
                            end_line: end_line_of(m, idx_const_node),
                        });
                    } else if is_all_caps(name) {
                        // ALL_CAPS constants at module level
                        let decl_node = name_cap.node.parent(); // variable_declarator
                        let lex_node = decl_node.and_then(|n| n.parent()); // lexical_declaration
                        let parent_node = lex_node.and_then(|n| n.parent()); // should be program
                        let is_module_level =
                            parent_node.map(|n| n.kind() == "program").unwrap_or(false);

                        if is_module_level {
                            symbols.push(ParsedSymbol {
                                name: name.to_string(),
                                kind: SymbolKind::Constant,
                                line,
                                signature: line_text(content, line).trim().to_string(),
                                parents: vec![],
                                end_line: end_line_of(m, idx_const_node),
                            });
                        }
                    }
                }
                continue;
            }

            if let Some(name_cap) = find_capture(m, idx_export_const_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if emitted_lines.insert((name.to_string(), line)) {
                    // Check for Vue Composition API calls
                    if let Some(api_kind) = is_composition_api_call(content, &name_cap.node) {
                        let kind = if api_kind == "macro" {
                            SymbolKind::Function
                        } else {
                            SymbolKind::Property
                        };
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind,
                            line,
                            signature: line_text(content, line).trim().to_string(),
                            parents: vec![],
                            end_line: end_line_of(m, idx_export_const_node),
                        });
                    } else if is_all_caps(name) {
                        // Export statement is always module-level
                        symbols.push(ParsedSymbol {
                            name: name.to_string(),
                            kind: SymbolKind::Constant,
                            line,
                            signature: line_text(content, line).trim().to_string(),
                            parents: vec![],
                            end_line: end_line_of(m, idx_export_const_node),
                        });
                    }
                }
                continue;
            }

            // === Ambient constants (export declare const) ===

            if let Some(name_cap) = find_capture(m, idx_export_ambient_const_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                if is_all_caps(name) && emitted_lines.insert((name.to_string(), line)) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![],
                        end_line: end_line_of(m, idx_export_ambient_const_node),
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
                        end_line: end_line_of(m, idx_namespace_node),
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
                        end_line: end_line_of(m, idx_export_namespace_node),
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
                        end_line: end_line_of(m, idx_import_node),
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
                        end_line: end_line_of(m, idx_decorator_node),
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
                        end_line: end_line_of(m, idx_decorator_call_node),
                    });
                }
                continue;
            }

            // === Class methods ===

            if emit_class_member(
                content,
                m,
                idx_method_name,
                idx_method_node,
                SymbolKind::Function,
                &mut symbols,
                &mut emitted_lines,
            ) {
                continue;
            }
            if emit_class_member(
                content,
                m,
                idx_private_method_name,
                idx_private_method_node,
                SymbolKind::Function,
                &mut symbols,
                &mut emitted_lines,
            ) {
                continue;
            }

            // === Class fields/properties ===

            if emit_class_member(
                content,
                m,
                idx_field_name,
                idx_field_node,
                SymbolKind::Property,
                &mut symbols,
                &mut emitted_lines,
            ) {
                continue;
            }
            if emit_class_member(
                content,
                m,
                idx_private_field_name,
                idx_private_field_node,
                SymbolKind::Property,
                &mut symbols,
                &mut emitted_lines,
            ) {
                continue;
            }

            // === Export default ===

            if let Some(val_cap) = find_capture(m, idx_export_default_value) {
                let node = &val_cap.node;
                let line = node_line(node);
                let sig = line_text(content, line).trim().to_string();

                match node.kind() {
                    // export default identifier;
                    "identifier" => {
                        let name = node_text(content, node);
                        if emitted_lines.insert((format!("default({})", name), line)) {
                            symbols.push(ParsedSymbol {
                                name: format!("default({})", name),
                                kind: SymbolKind::Object,
                                line,
                                signature: sig,
                                parents: vec![],
                                end_line: Some(node_end_line(node)),
                            });
                        }
                    }
                    // export default { ... }
                    "object" => {
                        if emitted_lines.insert(("default".to_string(), line)) {
                            symbols.push(ParsedSymbol {
                                name: "default".to_string(),
                                kind: SymbolKind::Object,
                                line,
                                signature: sig,
                                parents: vec![],
                                end_line: Some(node_end_line(node)),
                            });
                        }
                    }
                    // A higher-order call is named after what it wraps. Named after the
                    // wrapper, every file applying `injectIntl` claimed a definition of
                    // it, and the call on that line made `injectIntl` its own caller.
                    "call_expression" => match wrapped_value(*node) {
                        // export default memo(Button) / connect(mapState)(Button)
                        Some(wrapped) if wrapped.kind() == "identifier" => {
                            let name = format!("default({})", node_text(content, &wrapped));
                            if emitted_lines.insert((name.clone(), line)) {
                                symbols.push(ParsedSymbol {
                                    name,
                                    kind: SymbolKind::Object,
                                    line,
                                    signature: sig,
                                    parents: vec![],
                                    end_line: Some(node_end_line(node)),
                                });
                            }
                        }
                        // export default forwardRef((props, ref) => {})
                        Some(wrapped) => push_anonymous_default(
                            content,
                            &wrapped,
                            node,
                            sig,
                            &mut symbols,
                            &mut emitted_lines,
                        ),
                        // export default createRouter({...}) / defineComponent({...})
                        None => {
                            let name = node_text(content, &root_callee(*node));
                            if emitted_lines.insert((name.to_string(), line)) {
                                symbols.push(ParsedSymbol {
                                    name: name.to_string(),
                                    kind: SymbolKind::Function,
                                    line,
                                    signature: sig,
                                    parents: vec![],
                                    end_line: Some(node_end_line(node)),
                                });
                            }
                        }
                    },
                    // export default () => {} / function () {} / class {}
                    "arrow_function" | "function_expression" | "generator_function" | "class" => {
                        push_anonymous_default(
                            content,
                            node,
                            node,
                            sig,
                            &mut symbols,
                            &mut emitted_lines,
                        )
                    }
                    // A named `export default function f() {}` or `class F {}` is a
                    // declaration, not a value, and the declaration patterns emit it.
                    _ => {}
                }
                continue;
            }

            // === Abstract methods ===

            if emit_class_member(
                content,
                m,
                idx_abstract_method_name,
                idx_abstract_method_node,
                SymbolKind::Function,
                &mut symbols,
                &mut emitted_lines,
            ) {
                continue;
            }
        }

        Ok(symbols)
    }
}

impl TypeScriptParser {
    fn typescript_extract_refs(
        &self,
        content: &str,
        defined: &[ParsedSymbol],
        file_type: Option<FileType>,
    ) -> Result<Vec<ParsedRef>> {
        // Keep the existing generic extraction as a baseline; add AST-aware refs
        // for TypeScript-specific constructs it cannot see, then deduplicate.
        let mut refs = super::super::extract_references_for_lang(content, defined, file_type)?;
        let tree = parse_tree(content, &TS_LANGUAGE)?;

        let mut bindings: HashMap<String, Vec<AliasBinding>> = HashMap::new();
        collect_alias_bindings(content, &tree.root_node(), &mut bindings);
        collect_call_refs(content, &tree.root_node(), &bindings, &mut refs);
        dedup_refs(&mut refs);
        Ok(refs)
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
                        end_line: Some(node_end_line(&node_cap.node)),
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

/// Pushes the placeholder symbol for an anonymous default-exported function or
/// class `value`, ranged over `export` — the value itself, or the call that
/// wraps it.
fn push_anonymous_default(
    content: &str,
    value: &tree_sitter::Node,
    export: &tree_sitter::Node,
    signature: String,
    symbols: &mut Vec<ParsedSymbol>,
    emitted_lines: &mut HashSet<(String, usize)>,
) {
    let line = node_line(export);
    if !emitted_lines.insert((ANONYMOUS_DEFAULT_EXPORT.to_string(), line)) {
        return;
    }
    let (kind, parents) = if value.kind() == "class" {
        (SymbolKind::Class, extract_class_parents(content, value))
    } else {
        (SymbolKind::Function, vec![])
    };
    symbols.push(ParsedSymbol {
        name: ANONYMOUS_DEFAULT_EXPORT.to_string(),
        kind,
        line,
        signature,
        parents,
        end_line: Some(node_end_line(export)),
    });
}

/// What a higher-order call wraps: `Button` in `memo(Button)`,
/// `connect(mapState)(Button)`, `compose(a, b)(Button)` or
/// `memo(injectIntl(Button))`, and the inline function or class in
/// `forwardRef((props, ref) => …)`. It is the first argument, looked into
/// through nested calls. Any other first argument — `createRouter({ … })`,
/// `connect(null, actions)` — means the call builds a value rather than wraps
/// one, and there is nothing to name the export after.
fn wrapped_value(call: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let arguments = call
        .child_by_field_name("arguments")
        .filter(|arguments| arguments.kind() == "arguments")?;
    let mut cursor = arguments.walk();
    let first = arguments
        .named_children(&mut cursor)
        .find(|argument| argument.kind() != "comment")?;
    let first = unwrap_ref_expr(first);
    match first.kind() {
        "identifier"
        | "arrow_function"
        | "function_expression"
        | "generator_function"
        | "class" => Some(first),
        "call_expression" => wrapped_value(first),
        _ => None,
    }
}

/// The function a possibly curried call starts from: `connect` in
/// `connect(a)(b)`, `styled` in ``styled(Button)`…` ``.
fn root_callee(call: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    let mut callee = call;
    while callee.kind() == "call_expression" {
        match callee.child_by_field_name("function") {
            Some(function) => callee = function,
            None => break,
        }
    }
    callee
}

/// Placeholder name [`TypeScriptParser`] gives an anonymous `export default`
/// function or class. The parser never sees the file path the real name comes
/// from, so callers that know it pass the symbols through
/// [`name_default_export`].
///
/// The space keeps it apart from every real symbol: the query captures names
/// only as identifiers, and a method or field may well be called `default`.
pub const ANONYMOUS_DEFAULT_EXPORT: &str = "export default";

/// Renames the anonymous `export default` function or class after its module.
///
/// Every importer picks its own local name for such a value, so the one name
/// it reliably goes by is the module it is imported from. Without a name the
/// symbol is unfindable, and a call inside its body — which has a range, so
/// the owner lookup trusts it — is attributed to a symbol no one searches for.
///
/// A function is then classified the way a declaration with that name would
/// be, so an anonymous component in `Button.jsx` indexes like `function
/// Button()`. A path that yields no name falls back to `default`, the name
/// `export default {}` already gets.
pub fn name_default_export(symbols: &mut [ParsedSymbol], path: &str) {
    let name = default_export_name(path).unwrap_or_else(|| "default".to_string());
    for symbol in symbols
        .iter_mut()
        .filter(|s| s.name == ANONYMOUS_DEFAULT_EXPORT)
    {
        if symbol.kind == SymbolKind::Function {
            symbol.kind = classify_function_name(&name);
        }
        symbol.name = name.clone();
    }
}

/// The name a module is imported by: `hooks/useMap.js` → `useMap`. An
/// `index` file stands for its directory (`./Button` resolves to
/// `Button/index.jsx`), and everything from the first dot on is dropped
/// (`Button.test.jsx`, `index.web.js`, `types.d.ts`) — no identifier has one.
fn default_export_name(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    let module = path.file_name()?.to_str()?.split('.').next()?;
    let name = if module == "index" {
        path.parent()
            .and_then(index_directory_name)
            .unwrap_or(module)
    } else {
        module
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// The directory an `index` file in `dir` stands for. A build or source
/// directory only says where a package keeps the file — `pkg/dist/index.d.ts`
/// is what `import x from 'pkg'` loads — so the name comes from the nearest
/// directory above it. A package root in `node_modules` keeps its own name
/// even when it looks like one of them.
fn index_directory_name(dir: &std::path::Path) -> Option<&str> {
    let dirs: Vec<&str> = dir.iter().filter_map(|d| d.to_str()).collect();
    for (at, name) in dirs.iter().enumerate().rev() {
        let package_root = match at.checked_sub(1).map(|parent| dirs[parent]) {
            Some("node_modules") => true,
            Some(parent) if parent.starts_with('@') => at >= 2 && dirs[at - 2] == "node_modules",
            _ => false,
        };
        if package_root || !is_build_directory(name) {
            return Some(name);
        }
    }
    dirs.last().copied()
}

/// Directories named after a build output, module format or source layout
/// rather than after what they hold: `dist`, `lib`, `esm`, `src`, `types` and
/// variants such as `dist-types`, `lib.esm`, `types-ts3.8`, `es2015`, and the
/// `typesVersions` directories `ts3.4`, `ts4.0`.
fn is_build_directory(name: &str) -> bool {
    const DIRECTORIES: &[&str] = &[
        "build",
        "dist",
        "out",
        "lib",
        "src",
        "esm",
        "cjs",
        "es",
        "umd",
        "amd",
        "commonjs",
        "module",
        "esnext",
        "types",
        "typings",
        "declarations",
    ];
    const VARIANT_OF: &[&str] = &["build", "dist", "lib", "esm", "cjs", "types"];
    const VERSIONED: &[&str] = &["es", "esm", "fesm", "ts"];
    DIRECTORIES.contains(&name)
        || VARIANT_OF.iter().any(|base| {
            name.strip_prefix(base)
                .is_some_and(|rest| rest.starts_with(['-', '.', '_']))
        })
        || VERSIONED.iter().any(|base| {
            name.strip_prefix(base).is_some_and(|version| {
                version.starts_with(|c: char| c.is_ascii_digit())
                    && version.chars().all(|c| c.is_ascii_digit() || c == '.')
            })
        })
}

fn find_capture<'a>(
    m: &'a tree_sitter::QueryMatch<'a, 'a>,
    idx: Option<u32>,
) -> Option<&'a tree_sitter::QueryCapture<'a>> {
    let idx = idx?;
    m.captures.iter().find(|c| c.index == idx)
}

fn collect_alias_bindings(
    content: &str,
    node: &tree_sitter::Node,
    bindings: &mut HashMap<String, Vec<AliasBinding>>,
) {
    match node.kind() {
        "import_specifier" => {
            let imported = node
                .child_by_field_name("name")
                .map(|n| node_text(content, &n).to_string());
            let local = node
                .child_by_field_name("alias")
                .map(|n| node_text(content, &n).to_string())
                .or_else(|| imported.clone());
            if let (Some(imported), Some(local)) = (imported, local) {
                if imported != local {
                    add_alias_binding(
                        bindings,
                        local,
                        imported,
                        node_line(node),
                        scope_chain(node),
                    );
                }
            }
        }
        "variable_declarator" => {
            let local = node
                .child_by_field_name("name")
                .filter(|n| n.kind() == "identifier")
                .map(|n| node_text(content, &n).to_string());
            let value = node.child_by_field_name("value");
            if let (Some(local), Some(value)) = (local, value) {
                let line = node_line(node);
                let scope = scope_chain(node);
                if let Some(target) =
                    resolve_root_identifier(content, &value, line, &scope, bindings)
                {
                    if local != target {
                        add_alias_binding(bindings, local, target, line, scope);
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_alias_bindings(content, &child, bindings);
    }
}

fn collect_call_refs(
    content: &str,
    node: &tree_sitter::Node,
    bindings: &HashMap<String, Vec<AliasBinding>>,
    refs: &mut Vec<ParsedRef>,
) {
    if node.kind() == "call_expression" {
        if let Some(function_node) = node.child_by_field_name("function") {
            let line = node_line(node);
            let scope = scope_chain(node);
            if let Some(local_name) = direct_identifier_name(content, &function_node) {
                push_ref(refs, &local_name, line, content);
                if let Some(target) = resolve_binding_target(&local_name, line, &scope, bindings) {
                    if target != local_name {
                        push_ref(refs, &target, line, content);
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_call_refs(content, &child, bindings, refs);
    }
}

fn add_alias_binding(
    bindings: &mut HashMap<String, Vec<AliasBinding>>,
    local: String,
    target: String,
    declared_line: usize,
    scope_chain: ScopeChain,
) {
    bindings.entry(local).or_default().push(AliasBinding {
        declared_line,
        scope_chain,
        target,
    });
}

fn resolve_root_identifier(
    content: &str,
    node: &tree_sitter::Node,
    line: usize,
    scope: &ScopeChain,
    bindings: &HashMap<String, Vec<AliasBinding>>,
) -> Option<String> {
    let local_name = direct_identifier_name(content, node)?;
    Some(resolve_binding_target(&local_name, line, scope, bindings).unwrap_or(local_name))
}

fn resolve_binding_target(
    name: &str,
    line: usize,
    scope: &ScopeChain,
    bindings: &HashMap<String, Vec<AliasBinding>>,
) -> Option<String> {
    let mut current = name.to_string();
    let mut seen = HashSet::new();

    while seen.insert(current.clone()) {
        let next = bindings
            .get(&current)
            .and_then(|candidates| best_binding(candidates, line, scope))
            .map(|binding| binding.target.clone());

        match next {
            Some(ref target) if target != &current => current = target.clone(),
            _ => break,
        }
    }

    if current == name {
        None
    } else {
        Some(current)
    }
}

fn best_binding<'a>(
    bindings: &'a [AliasBinding],
    line: usize,
    scope: &ScopeChain,
) -> Option<&'a AliasBinding> {
    bindings
        .iter()
        .filter(|binding| {
            binding.declared_line <= line && scope_starts_with(scope, &binding.scope_chain)
        })
        .max_by_key(|binding| (binding.scope_chain.len(), binding.declared_line))
}

fn scope_starts_with(scope: &ScopeChain, prefix: &ScopeChain) -> bool {
    prefix.len() <= scope.len() && scope.iter().zip(prefix.iter()).all(|(a, b)| a == b)
}

fn scope_chain(node: &tree_sitter::Node) -> ScopeChain {
    let mut chain = Vec::new();
    let mut current = Some(*node);

    while let Some(node) = current {
        if is_scope_node(node.kind()) {
            chain.push((node.start_byte(), node.end_byte()));
        }
        current = node.parent();
    }

    chain.reverse();
    chain
}

fn is_scope_node(kind: &str) -> bool {
    matches!(
        kind,
        "program"
            | "statement_block"
            | "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "method_definition"
            | "generator_function"
            | "class_static_block"
    )
}

fn direct_identifier_name(content: &str, node: &tree_sitter::Node) -> Option<String> {
    let node = unwrap_ref_expr(*node);
    if node.kind() == "identifier" {
        Some(node_text(content, &node).to_string())
    } else {
        None
    }
}

fn unwrap_ref_expr(mut node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    loop {
        let next = match node.kind() {
            "instantiation_expression" => node
                .child_by_field_name("expression")
                .or_else(|| node.named_child(0)),
            "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "type_assertion"
            | "non_null_expression" => node.named_child(0),
            _ => None,
        };

        match next {
            Some(inner) => node = inner,
            None => return node,
        }
    }
}

fn push_ref(refs: &mut Vec<ParsedRef>, name: &str, line: usize, content: &str) {
    let context = content
        .lines()
        .nth(line.saturating_sub(1))
        .map(str::trim)
        .unwrap_or("");
    refs.push(ParsedRef {
        name: name.to_string(),
        line,
        context: truncate_context(context),
    });
}

fn dedup_refs(refs: &mut Vec<ParsedRef>) {
    let mut seen = HashSet::new();
    refs.retain(|r| seen.insert((r.name.clone(), r.line, r.context.clone())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = "export class UserService extends BaseService implements IUserService {\n}\n\nclass ChildClass extends ParentClass {\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ChildClass" && s.parents.iter().any(|(p, _)| p == "ParentClass")));
    }

    #[test]
    fn test_parse_interface() {
        let content = "interface User {\n    id: string;\n}\n\nexport interface IUserService extends IService {\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "IUserService" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = "type UserId = string;\nexport type UserMap = Map<string, User>;\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserId" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserMap" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_enum() {
        let content = "enum Status {\n    Active,\n    Inactive,\n}\n\nexport const enum Direction {\n    Up,\n    Down,\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Direction" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_functions() {
        let content = "function handleRequest(req: Request): Response {\n    return new Response();\n}\n\nexport async function fetchUser(id: string): Promise<User> {\n    return fetch(`/users/${id}`);\n}\n\nconst processData = (data: Data) => {\n    return data;\n};\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "handleRequest" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "fetchUser" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "processData" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_react_component() {
        let content = "const Button: React.FC<ButtonProps> = ({ children, onClick }) => {\n    return <button onClick={onClick}>{children}</button>;\n};\n\nexport function UserCard({ user }: UserCardProps) {\n    return <div>{user.name}</div>;\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Button" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserCard" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_react_hooks() {
        let content = "function useAuth() {\n    const [user, setUser] = useState(null);\n    return { user };\n}\n\nexport const useCounter = () => {\n    return { count: 0 };\n};\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "useAuth" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "useCounter" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_constants() {
        let content = "const API_URL = 'https://api.example.com';\nexport const MAX_RETRIES = 3;\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "API_URL" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_extract_refs_generic_calls_and_aliases() {
        let content = r#"
import { targetFn as bus } from './bus';
import { targetFn } from './bus';

export const baseline = targetFn();
export const generic = targetFn<{ id: string }>();
export const aliased = bus();

const localBus = targetFn<{ id: string }>;

export const c1 = localBus().run({ id: 'c-1' });
export const c2 = localBus().run({ id: 'c-2' });
export const c3 = localBus().run({ id: 'c-3' });
"#;

        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        let refs = TYPESCRIPT_PARSER
            .extract_refs_for_lang(content, &symbols, FileType::TypeScript)
            .unwrap();

        let target_lines: Vec<usize> = refs
            .iter()
            .filter(|r| r.name == "targetFn")
            .map(|r| r.line)
            .collect();

        for line in [5, 6, 7, 11, 12, 13] {
            assert!(
                target_lines.contains(&line),
                "expected targetFn usage on line {line}; got refs: {:?}",
                refs.iter()
                    .map(|r| (r.name.as_str(), r.line))
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            refs.iter().any(|r| r.name == "bus" && r.line == 7),
            "aliased local name should still be indexed"
        );
        assert!(
            refs.iter().filter(|r| r.name == "localBus").count() >= 3,
            "rebound local name should still be indexed"
        );
    }

    #[test]
    fn test_parse_namespace() {
        let content = "namespace Utils {\n    export function helper() {}\n}\n\nexport namespace Types {\n    export interface User {}\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Utils" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Types" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_decorators() {
        let content = "@Controller('users')\nexport class UserController {\n    @Get(':id')\n    getUser(@Param('id') id: string) {}\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "@Controller" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "@Get" && s.kind == SymbolKind::Annotation));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "constructor" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "getUser" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "validate" && s.kind == SymbolKind::Function));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "value" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "create" && s.kind == SymbolKind::Function));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "age" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "count" && s.kind == SymbolKind::Property));
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "process" && s.kind == SymbolKind::Function));
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
        assert!(!symbols
            .iter()
            .any(|s| s.name == "method" && s.kind == SymbolKind::Function));
        assert!(!symbols
            .iter()
            .any(|s| s.name == "prop" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_dts_ambient_declarations() {
        // .d.ts files use "declare" keyword (ambient declarations)
        let content = r#"
import type { ToasterPublicMethods } from "../types.js";
export declare function useToaster(): ToasterPublicMethods;
export declare class Theme {}
export declare interface ThemeProps {
    color: string;
}
export declare type ThemeColor = "light" | "dark";
export declare enum Direction {
    Up = "up",
    Down = "down",
}
export declare const MAX_RETRIES: number;
export declare namespace Utils {
    function helper(): void;
}
declare function internalHelper(): void;
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        // declare function
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "useToaster" && s.kind == SymbolKind::Function),
            "useToaster not found; symbols: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(symbols
            .iter()
            .any(|s| s.name == "internalHelper" && s.kind == SymbolKind::Function));
        // declare class
        assert!(symbols
            .iter()
            .any(|s| s.name == "Theme" && s.kind == SymbolKind::Class));
        // declare interface
        assert!(symbols
            .iter()
            .any(|s| s.name == "ThemeProps" && s.kind == SymbolKind::Interface));
        // declare type
        assert!(symbols
            .iter()
            .any(|s| s.name == "ThemeColor" && s.kind == SymbolKind::TypeAlias));
        // declare enum
        assert!(symbols
            .iter()
            .any(|s| s.name == "Direction" && s.kind == SymbolKind::Enum));
        // declare const (ALL_CAPS)
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
        // declare namespace
        assert!(symbols
            .iter()
            .any(|s| s.name == "Utils" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_export_default_identifier() {
        let content = "const router = createRouter({ routes })\n\nexport default router;\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "default(router)" && s.kind == SymbolKind::Object),
            "should find 'default(router)'; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_export_default_object() {
        let content = "export default {\n  install(app) {\n    app.component('MyComponent', MyComponent)\n  }\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "default" && s.kind == SymbolKind::Object),
            "should find 'default' as object; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_export_default_call() {
        let content =
            "export default createRouter({\n  history: createWebHistory(),\n  routes,\n})\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "createRouter" && s.kind == SymbolKind::Function),
            "should find 'createRouter'; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
    }

    fn anonymous_default(content: &str) -> Vec<(SymbolKind, usize, Option<usize>)> {
        TYPESCRIPT_PARSER
            .parse_symbols(content)
            .unwrap()
            .into_iter()
            .filter(|s| s.name == ANONYMOUS_DEFAULT_EXPORT)
            .map(|s| (s.kind, s.line, s.end_line))
            .collect()
    }

    #[test]
    fn anonymous_default_export_gets_a_ranged_symbol() {
        let cases = [
            (
                "const a = 1;\nexport default ({ a }) => {\n  return a;\n};\n",
                SymbolKind::Function,
            ),
            (
                "export default async () => {\n  await x();\n}\n",
                SymbolKind::Function,
            ),
            (
                "export default function () {\n  x();\n}\n",
                SymbolKind::Function,
            ),
            (
                "export default async function () {\n  x();\n}\n",
                SymbolKind::Function,
            ),
            (
                "export default function* () {\n  yield 1;\n}\n",
                SymbolKind::Function,
            ),
            ("export default class {\n  run() {}\n}\n", SymbolKind::Class),
        ];
        for (content, kind) in cases {
            let start = content
                .lines()
                .position(|l| l.starts_with("export"))
                .unwrap()
                + 1;
            let end = content.lines().count();
            assert_eq!(
                anonymous_default(content),
                vec![(kind, start, Some(end))],
                "{content}"
            );
        }
    }

    #[test]
    fn anonymous_default_class_keeps_its_parents() {
        let content = "export default class extends Component {\n  render() {}\n}\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.name == ANONYMOUS_DEFAULT_EXPORT)
            .unwrap();
        assert_eq!(
            class.parents,
            vec![("Component".to_string(), "extends".to_string())]
        );
        assert!(symbols.iter().any(|s| s.name == "render"));
    }

    #[test]
    fn named_default_export_is_not_duplicated() {
        for content in [
            "export default function useMap() {\n  x();\n}\n",
            "export default class Widget {\n  run() {}\n}\n",
            "export default function* gen() {\n  yield 1;\n}\n",
        ] {
            assert!(anonymous_default(content).is_empty(), "{content}");
        }
        let symbols = TYPESCRIPT_PARSER
            .parse_symbols("export default function useMap() {\n  x();\n}\n")
            .unwrap();
        assert_eq!(
            symbols.iter().filter(|s| s.name == "useMap").count(),
            1,
            "{symbols:?}"
        );
    }

    #[test]
    fn default_export_is_named_after_its_module() {
        let rename = |content: &str, path: &str| {
            let mut symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
            name_default_export(&mut symbols, path);
            symbols
                .into_iter()
                .map(|s| (s.name, s.kind))
                .collect::<Vec<_>>()
        };
        let arrow = "export default () => {\n  x();\n};\n";
        assert_eq!(
            rename(arrow, "src/hooks/useMap.js"),
            vec![("useMap".to_string(), SymbolKind::Function)]
        );
        assert_eq!(
            rename(arrow, "src/components/Button/index.jsx"),
            vec![("Button".to_string(), SymbolKind::Class)]
        );
        assert_eq!(
            rename(arrow, "src/Button.web.tsx"),
            vec![("Button".to_string(), SymbolKind::Class)]
        );
        assert_eq!(
            rename(arrow, "index.js"),
            vec![("index".to_string(), SymbolKind::Function)]
        );
        assert_eq!(
            rename("export default class {}\n", "src/api/client.ts"),
            vec![("client".to_string(), SymbolKind::Class)]
        );
        assert_eq!(
            rename(arrow, "src/.hidden.js"),
            vec![("default".to_string(), SymbolKind::Function)]
        );
    }

    #[test]
    fn index_default_export_is_named_past_build_directories() {
        for (path, expected) in [
            ("node_modules/stylish/dist/index.d.ts", "stylish"),
            ("node_modules/stylish/lib/index.d.ts", "stylish"),
            ("node_modules/@scope/stylish/dist/esm/index.d.ts", "stylish"),
            ("node_modules/stylish/dist-types/index.d.ts", "stylish"),
            ("node_modules/stylish/types-ts3.8/index.d.ts", "stylish"),
            ("node_modules/stylish/ts3.4/index.d.ts", "stylish"),
            ("node_modules/stylish/dist.es2015/index.d.ts", "stylish"),
            ("node_modules/stylish/lib/es5/index.d.ts", "stylish"),
            ("packages/button/src/index.ts", "button"),
            // An entry point is not a build directory.
            ("node_modules/stylish/compat/index.d.ts", "compat"),
            // A package keeps its name even when it reads like a build directory.
            ("node_modules/@scope/types/index.d.ts", "types"),
            ("node_modules/lib/dist/index.d.ts", "lib"),
            // With nothing above it, the build directory is still better than `index`.
            ("src/index.js", "src"),
            ("src/components/Button/index.jsx", "Button"),
        ] {
            let mut symbols = TYPESCRIPT_PARSER
                .parse_symbols("export default () => {};\n")
                .unwrap();
            name_default_export(&mut symbols, path);
            assert_eq!(symbols[0].name, expected, "{path}");
        }
    }

    #[test]
    fn default_export_naming_leaves_other_default_symbols_alone() {
        for (content, expected) in [
            ("export default {\n  a: 1,\n};\n", "default"),
            (
                "const router = 1;\nexport default router;\n",
                "default(router)",
            ),
            ("export default createRouter({});\n", "createRouter"),
        ] {
            let mut symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
            name_default_export(&mut symbols, "src/router.js");
            assert!(
                symbols.iter().any(|s| s.name == expected),
                "{content}: {symbols:?}"
            );
            assert!(!symbols.iter().any(|s| s.name == "router"), "{symbols:?}");
        }
    }

    fn ranged_symbols(content: &str) -> Vec<(String, SymbolKind, usize, Option<usize>)> {
        TYPESCRIPT_PARSER
            .parse_symbols(content)
            .unwrap()
            .into_iter()
            .map(|s| (s.name, s.kind, s.line, s.end_line))
            .collect()
    }

    #[test]
    fn hoc_default_export_is_named_after_the_wrapped_identifier() {
        for (content, wrapped, end) in [
            ("export default injectIntl(Header);\n", "Header", 1),
            ("export default memo(Button, areEqual);\n", "Button", 1),
            ("export default React.memo(injectIntl(Card));\n", "Card", 1),
            (
                "export default connect(mapState, mapDispatch)(Page);\n",
                "Page",
                1,
            ),
            (
                "export default connect((state) => ({ a: state.a }))(Page);\n",
                "Page",
                1,
            ),
            (
                "export default compose(\n  withRouter,\n  connect\n)(Page);\n",
                "Page",
                4,
            ),
        ] {
            assert_eq!(
                ranged_symbols(content),
                vec![(
                    format!("default({wrapped})"),
                    SymbolKind::Object,
                    1,
                    Some(end)
                )],
                "{content}"
            );
        }
    }

    #[test]
    fn hoc_default_export_leaves_the_wrapped_declaration_alone() {
        let content = "const Header = () => null;\n\nexport default injectIntl(Header);\n";
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        let named = symbols
            .iter()
            .map(|s| (s.name.as_str(), s.line))
            .collect::<Vec<_>>();
        assert_eq!(named, vec![("Header", 1), ("default(Header)", 3)]);
    }

    #[test]
    fn hoc_wrapping_an_inline_value_is_an_anonymous_default() {
        let cases = [
            (
                "export default forwardRef((props, ref) => {\n  return render(ref);\n});\n",
                SymbolKind::Function,
            ),
            (
                "export default injectIntl(({ intl }) => (\n  <div>{intl.locale}</div>\n));\n",
                SymbolKind::Function,
            ),
            (
                "export default memo(function () {\n  return null;\n});\n",
                SymbolKind::Function,
            ),
            (
                "export default observer(class extends Component {\n  render() {}\n});\n",
                SymbolKind::Class,
            ),
        ];
        for (content, kind) in cases {
            assert_eq!(
                anonymous_default(content),
                vec![(kind, 1, Some(3))],
                "{content}"
            );
        }
        let symbols = TYPESCRIPT_PARSER.parse_symbols(cases[3].0).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.name == ANONYMOUS_DEFAULT_EXPORT)
            .unwrap();
        assert_eq!(
            class.parents,
            vec![("Component".to_string(), "extends".to_string())]
        );

        let mut symbols = TYPESCRIPT_PARSER.parse_symbols(cases[1].0).unwrap();
        name_default_export(&mut symbols, "src/components/Card.jsx");
        assert_eq!(symbols[0].name, "Card");
        assert_eq!(symbols[0].kind, SymbolKind::Class);
    }

    #[test]
    fn default_export_of_a_built_value_keeps_the_callee_name() {
        for (content, name) in [
            ("export default createRouter({ routes });\n", "createRouter"),
            ("export default connect(null, actions);\n", "connect"),
            ("export default createStore();\n", "createStore"),
            (
                "export default defineStore('auth', () => ({}));\n",
                "defineStore",
            ),
            (
                "export default styled(Button)`\n  color: red;\n`;\n",
                "styled",
            ),
        ] {
            let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
            assert_eq!(
                symbols
                    .iter()
                    .map(|s| (s.name.as_str(), s.kind))
                    .collect::<Vec<_>>(),
                vec![(name, SymbolKind::Function)],
                "{content}"
            );
        }
    }

    #[test]
    fn default_export_naming_keeps_members_called_default() {
        let content = "class Config {\n  default = 1;\n  static default() {}\n}\n\nexport default () => new Config();\n";
        let mut symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        name_default_export(&mut symbols, "src/config.js");
        let named = symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect::<Vec<_>>();
        assert!(
            named.contains(&("default", SymbolKind::Property, 2)),
            "{named:?}"
        );
        assert!(
            named.contains(&("default", SymbolKind::Function, 3)),
            "{named:?}"
        );
        assert!(
            named.contains(&("config", SymbolKind::Function, 6)),
            "{named:?}"
        );
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
        assert!(symbols
            .iter()
            .any(|s| s.name == "#secret" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "#process" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_vue_composition_api_in_ts() {
        let content = r#"
const count = ref(0)
const items = reactive<Item[]>([])
const doubled = computed(() => count.value * 2)
const name = shallowRef('hello')
const data = readonly(state)
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "count" && s.kind == SymbolKind::Property),
            "should find 'count' as ref property; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "items" && s.kind == SymbolKind::Property),
            "should find 'items' as reactive property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "doubled" && s.kind == SymbolKind::Property),
            "should find 'doubled' as computed property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "name" && s.kind == SymbolKind::Property),
            "should find 'name' as shallowRef property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "data" && s.kind == SymbolKind::Property),
            "should find 'data' as readonly property"
        );
    }

    #[test]
    fn test_parse_pinia_store_in_ts() {
        // Pinia store defined in a .ts file
        let content = r#"
export const useAuthStore = defineStore('auth', () => {
  const user = ref(null)
  const token = ref('')
  const isAuthenticated = computed(() => !!user.value)

  async function login(email: string, password: string) {
    const response = await api.post('/login', { email, password })
    user.value = response.data
  }

  return { user, token, isAuthenticated, login }
})
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        // Store itself (arrow function from defineStore)
        assert!(
            symbols.iter().any(|s| s.name == "useAuthStore"),
            "should find 'useAuthStore'; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        // Reactive state inside store
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "user" && s.kind == SymbolKind::Property),
            "should find 'user' as ref property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "token" && s.kind == SymbolKind::Property),
            "should find 'token' as ref property"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "isAuthenticated" && s.kind == SymbolKind::Property),
            "should find 'isAuthenticated' as computed property"
        );
        // Actions (functions inside store)
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "login" && s.kind == SymbolKind::Function),
            "should find 'login' function"
        );
    }

    #[test]
    fn test_parse_define_macros_in_ts() {
        let content = r#"
const props = defineProps<{ msg: string }>()
const emit = defineEmits<{ click: [] }>()
const model = defineModel<string>()
export const useTaskStore = defineStore('tasks', () => { return {} })
"#;
        let symbols = TYPESCRIPT_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "props" && s.kind == SymbolKind::Function),
            "should find 'props' from defineProps as function; got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "emit" && s.kind == SymbolKind::Function),
            "should find 'emit' from defineEmits as function"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "model" && s.kind == SymbolKind::Function),
            "should find 'model' from defineModel as function"
        );
        assert!(
            symbols.iter().any(|s| s.name == "useTaskStore"),
            "should find 'useTaskStore' from defineStore"
        );
    }
}
