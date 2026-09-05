//! Tree-sitter based C++ parser
//!
//! Parses C and C++ source files to extract:
//! - Classes and structs (including template classes)
//! - Functions (including template functions and JNI exports)
//! - Method definitions (ClassName::MethodName)
//! - Namespaces (including nested C++17 syntax)
//! - Enums (including enum class)
//! - Type aliases (typedef and using)
//! - Function-like macros (#define)
//! - Includes (#include)

use anyhow::Result;
use std::collections::HashMap;
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_line, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static CPP_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_cpp::LANGUAGE.into());

static CPP_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&CPP_LANGUAGE, include_str!("queries/cpp.scm"))
        .expect("Failed to compile C++ tree-sitter query")
});

pub static CPP_PARSER: CppParser = CppParser;

pub struct CppParser;

impl LanguageParser for CppParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &CPP_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*CPP_QUERY;

        // Build capture name -> index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        // Class/struct captures
        let idx_class_name = idx("class_name");
        let idx_class_node = idx("class_node");
        let idx_struct_name = idx("struct_name");
        let idx_struct_node = idx("struct_node");
        let idx_template_class_name = idx("template_class_name");
        let idx_template_class_node = idx("template_class_node");
        let idx_template_struct_name = idx("template_struct_name");
        let idx_template_struct_node = idx("template_struct_node");

        // Function captures
        let idx_func_name = idx("func_name");
        let idx_template_func_name = idx("template_func_name");
        let idx_method_class = idx("method_class");
        let idx_method_name = idx("method_name");
        let idx_template_method_class = idx("template_method_class");
        let idx_template_method_name = idx("template_method_name");
        let idx_destructor_class = idx("destructor_class");
        let idx_destructor_name = idx("destructor_name");

        // Other captures
        let idx_namespace_name = idx("namespace_name");
        let idx_enum_name = idx("enum_name");
        let idx_enum_value = idx("enum_value");
        let idx_typedef_name = idx("typedef_name");
        let idx_typedef_node = idx("typedef_node");
        let idx_using_alias_name = idx("using_alias_name");
        let idx_macro_name = idx("macro_name");
        let idx_include_path = idx("include_path");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // --- Class with body (not forward declaration) ---
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                if find_capture(m, idx_class_node).is_some() {
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    let parents = extract_base_classes(content, &name_cap.node);
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

            // --- Struct with body ---
            if let Some(name_cap) = find_capture(m, idx_struct_name) {
                if find_capture(m, idx_struct_node).is_some() {
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    let parents = extract_base_classes(content, &name_cap.node);
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

            // --- Template class with body ---
            if let Some(name_cap) = find_capture(m, idx_template_class_name) {
                if find_capture(m, idx_template_class_node).is_some() {
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    let parents = extract_base_classes(content, &name_cap.node);
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

            // --- Template struct with body ---
            if let Some(name_cap) = find_capture(m, idx_template_struct_name) {
                if find_capture(m, idx_template_struct_node).is_some() {
                    let name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    let parents = extract_base_classes(content, &name_cap.node);
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

            // --- Method definition: ClassName::MethodName ---
            if let Some(class_cap) = find_capture(m, idx_method_class) {
                if let Some(name_cap) = find_capture(m, idx_method_name) {
                    let class_name = node_text(content, &class_cap.node);
                    let method_name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);

                    // Check for JNI pattern: JNIEXPORT ... JNICALL Java_pkg_Class_method
                    let sig_line = line_text(content, line).trim().to_string();
                    if is_jni_function(&sig_line) {
                        // Extract the JNI function name from the signature
                        if let Some(jni_name) = extract_jni_method_name(&sig_line) {
                            symbols.push(ParsedSymbol {
                                name: jni_name,
                                kind: SymbolKind::Function,
                                line,
                                signature: sig_line,
                                parents: vec![],
                            });
                            continue;
                        }
                    }

                    if !is_reserved_word(method_name) {
                        symbols.push(ParsedSymbol {
                            name: method_name.to_string(),
                            kind: SymbolKind::Function,
                            line,
                            signature: sig_line,
                            parents: vec![(class_name.to_string(), "member".to_string())],
                        });
                    }
                }
                continue;
            }

            // --- Template method definition: ClassName::MethodName ---
            if let Some(class_cap) = find_capture(m, idx_template_method_class) {
                if let Some(name_cap) = find_capture(m, idx_template_method_name) {
                    let class_name = node_text(content, &class_cap.node);
                    let method_name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    if !is_reserved_word(method_name) {
                        symbols.push(ParsedSymbol {
                            name: method_name.to_string(),
                            kind: SymbolKind::Function,
                            line,
                            signature: line_text(content, line).trim().to_string(),
                            parents: vec![(class_name.to_string(), "member".to_string())],
                        });
                    }
                }
                continue;
            }

            // --- Destructor definition: ClassName::~ClassName ---
            if let Some(class_cap) = find_capture(m, idx_destructor_class) {
                if let Some(name_cap) = find_capture(m, idx_destructor_name) {
                    let class_name = node_text(content, &class_cap.node);
                    let dtor_name = node_text(content, &name_cap.node);
                    let line = node_line(&name_cap.node);
                    symbols.push(ParsedSymbol {
                        name: dtor_name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents: vec![(class_name.to_string(), "member".to_string())],
                    });
                }
                continue;
            }

            // --- Template function ---
            if let Some(cap) = find_capture(m, idx_template_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                if !is_reserved_word(name) {
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

            // --- Regular function ---
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);

                // Check for JNI pattern in signature line
                let sig_line = line_text(content, line).trim().to_string();
                if is_jni_function(&sig_line) {
                    if let Some(jni_name) = extract_jni_method_name(&sig_line) {
                        symbols.push(ParsedSymbol {
                            name: jni_name,
                            kind: SymbolKind::Function,
                            line,
                            signature: sig_line,
                            parents: vec![],
                        });
                        continue;
                    }
                }

                if !is_reserved_word(name) {
                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Function,
                        line,
                        signature: sig_line,
                        parents: vec![],
                    });
                }
                continue;
            }

            // --- Namespace ---
            if let Some(cap) = find_capture(m, idx_namespace_name) {
                let raw_name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let sig = line_text(content, line).trim().to_string();

                if !raw_name.is_empty() {
                    if raw_name.contains("::") {
                        for part in raw_name.split("::").filter(|part| !part.is_empty()) {
                            symbols.push(ParsedSymbol {
                                name: part.to_string(),
                                kind: SymbolKind::Package,
                                line,
                                signature: sig.clone(),
                                parents: vec![],
                            });
                        }
                    } else {
                        symbols.push(ParsedSymbol {
                            name: raw_name.to_string(),
                            kind: SymbolKind::Package,
                            line,
                            signature: sig,
                            parents: vec![],
                        });
                    }
                }
                continue;
            }

            // --- Enum ---
            if let Some(cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });

                if let Some(value_cap) = find_capture(m, idx_enum_value) {
                    let value = node_text(content, &value_cap.node);
                    let value_line = node_line(&value_cap.node);
                    symbols.push(ParsedSymbol {
                        name: value.to_string(),
                        kind: SymbolKind::Constant,
                        line: value_line,
                        signature: line_text(content, value_line).trim().to_string(),
                        parents: vec![(name.to_string(), "member".to_string())],
                    });
                }
                continue;
            }

            // --- Typedef (simple: typedef ... Name;) ---
            if let Some(cap) = find_capture(m, idx_typedef_name) {
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

            // --- Typedef (complex: function pointers, etc.) ---
            // This catches type_definition nodes not handled by the simple pattern above
            if let Some(cap) = find_capture(m, idx_typedef_node) {
                // Skip if this was already handled by the simple typedef_name capture
                if find_capture(m, idx_typedef_name).is_none() {
                    let line = node_line(&cap.node);
                    if let Some(name) = extract_typedef_name(&cap.node, content) {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            line,
                            signature: line_text(content, line).trim().to_string(),
                            parents: vec![],
                        });
                    }
                }
                continue;
            }

            // --- Using alias ---
            if let Some(cap) = find_capture(m, idx_using_alias_name) {
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

            // --- Function-like macro ---
            if let Some(cap) = find_capture(m, idx_macro_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Constant,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // --- Include ---
            if let Some(cap) = find_capture(m, idx_include_path) {
                let raw_path = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                // Strip quotes and angle brackets
                let path = raw_path
                    .trim_matches('"')
                    .trim_start_matches('<')
                    .trim_end_matches('>');
                // Extract file name from path (last component)
                let name = path.rsplit('/').next().unwrap_or(path);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Import,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![(path.to_string(), "from".to_string())],
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

pub fn collect_qualified_names(content: &str) -> Result<HashMap<(String, usize, String), String>> {
    let tree = parse_tree(content, &CPP_LANGUAGE)?;
    let mut qualified = HashMap::new();
    let mut cursor = QueryCursor::new();
    let query = &*CPP_QUERY;

    let capture_names = query.capture_names();
    let idx = |name: &str| -> Option<u32> {
        capture_names
            .iter()
            .position(|n| *n == name)
            .map(|i| i as u32)
    };

    let idx_class_name = idx("class_name");
    let idx_class_node = idx("class_node");
    let idx_struct_name = idx("struct_name");
    let idx_struct_node = idx("struct_node");
    let idx_template_class_name = idx("template_class_name");
    let idx_template_class_node = idx("template_class_node");
    let idx_template_struct_name = idx("template_struct_name");
    let idx_template_struct_node = idx("template_struct_node");
    let idx_func_name = idx("func_name");
    let idx_template_func_name = idx("template_func_name");
    let idx_method_class = idx("method_class");
    let idx_method_name = idx("method_name");
    let idx_template_method_class = idx("template_method_class");
    let idx_template_method_name = idx("template_method_name");
    let idx_destructor_class = idx("destructor_class");
    let idx_destructor_name = idx("destructor_name");
    let idx_namespace_name = idx("namespace_name");
    let idx_enum_name = idx("enum_name");
    let idx_enum_value = idx("enum_value");
    let idx_typedef_name = idx("typedef_name");
    let idx_typedef_node = idx("typedef_node");
    let idx_using_alias_name = idx("using_alias_name");

    let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

    while let Some(m) = matches.next() {
        if let Some(name_cap) = find_capture(m, idx_class_name) {
            if find_capture(m, idx_class_node).is_some() {
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Class,
                    node_line(&name_cap.node),
                    node_text(content, &name_cap.node),
                    node_text_with_namespace(content, &name_cap.node),
                );
            }
            continue;
        }

        if let Some(name_cap) = find_capture(m, idx_struct_name) {
            if find_capture(m, idx_struct_node).is_some() {
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Class,
                    node_line(&name_cap.node),
                    node_text(content, &name_cap.node),
                    node_text_with_namespace(content, &name_cap.node),
                );
            }
            continue;
        }

        if let Some(name_cap) = find_capture(m, idx_template_class_name) {
            if find_capture(m, idx_template_class_node).is_some() {
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Class,
                    node_line(&name_cap.node),
                    node_text(content, &name_cap.node),
                    node_text_with_namespace(content, &name_cap.node),
                );
            }
            continue;
        }

        if let Some(name_cap) = find_capture(m, idx_template_struct_name) {
            if find_capture(m, idx_template_struct_node).is_some() {
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Class,
                    node_line(&name_cap.node),
                    node_text(content, &name_cap.node),
                    node_text_with_namespace(content, &name_cap.node),
                );
            }
            continue;
        }

        if let Some(class_cap) = find_capture(m, idx_method_class) {
            if let Some(name_cap) = find_capture(m, idx_method_name) {
                let class_name = node_text_with_namespace(content, &class_cap.node);
                let method_name = node_text(content, &name_cap.node);
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Function,
                    node_line(&name_cap.node),
                    method_name,
                    format!("{class_name}::{method_name}"),
                );
            }
            continue;
        }

        if let Some(class_cap) = find_capture(m, idx_template_method_class) {
            if let Some(name_cap) = find_capture(m, idx_template_method_name) {
                let class_name = node_text_with_namespace(content, &class_cap.node);
                let method_name = node_text(content, &name_cap.node);
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Function,
                    node_line(&name_cap.node),
                    method_name,
                    format!("{class_name}::{method_name}"),
                );
            }
            continue;
        }

        if let Some(class_cap) = find_capture(m, idx_destructor_class) {
            if let Some(name_cap) = find_capture(m, idx_destructor_name) {
                let class_name = node_text_with_namespace(content, &class_cap.node);
                let method_name = node_text(content, &name_cap.node);
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Function,
                    node_line(&name_cap.node),
                    method_name,
                    format!("{class_name}::{method_name}"),
                );
            }
            continue;
        }

        if let Some(cap) = find_capture(m, idx_template_func_name) {
            remember_qualified_name(
                &mut qualified,
                SymbolKind::Function,
                node_line(&cap.node),
                node_text(content, &cap.node),
                node_text_with_namespace(content, &cap.node),
            );
            continue;
        }

        if let Some(cap) = find_capture(m, idx_func_name) {
            remember_qualified_name(
                &mut qualified,
                SymbolKind::Function,
                node_line(&cap.node),
                node_text(content, &cap.node),
                node_text_with_namespace(content, &cap.node),
            );
            continue;
        }

        if let Some(cap) = find_capture(m, idx_namespace_name) {
            let raw_name = node_text(content, &cap.node);
            let line = node_line(&cap.node);
            if raw_name.contains("::") {
                let mut current = Vec::new();
                for part in raw_name.split("::").filter(|part| !part.is_empty()) {
                    current.push(part);
                    remember_qualified_name(
                        &mut qualified,
                        SymbolKind::Package,
                        line,
                        part,
                        current.join("::"),
                    );
                }
            } else {
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Package,
                    line,
                    raw_name,
                    node_text_with_namespace(content, &cap.node),
                );
            }
            continue;
        }

        if let Some(cap) = find_capture(m, idx_enum_name) {
            remember_qualified_name(
                &mut qualified,
                SymbolKind::Enum,
                node_line(&cap.node),
                node_text(content, &cap.node),
                node_text_with_namespace(content, &cap.node),
            );

            if let Some(value_cap) = find_capture(m, idx_enum_value) {
                let enum_name = node_text(content, &cap.node);
                let value_name = node_text(content, &value_cap.node);
                let qualified_value = qualify_enum_value(content, &cap.node, enum_name, value_name);
                remember_qualified_name(
                    &mut qualified,
                    SymbolKind::Constant,
                    node_line(&value_cap.node),
                    value_name,
                    qualified_value,
                );
            }
            continue;
        }

        if let Some(cap) = find_capture(m, idx_typedef_name) {
            remember_qualified_name(
                &mut qualified,
                SymbolKind::TypeAlias,
                node_line(&cap.node),
                node_text(content, &cap.node),
                node_text_with_namespace(content, &cap.node),
            );
            continue;
        }

        if let Some(cap) = find_capture(m, idx_typedef_node) {
            if find_capture(m, idx_typedef_name).is_none() {
                if let Some(name) = extract_typedef_name(&cap.node, content) {
                    remember_qualified_name(
                        &mut qualified,
                        SymbolKind::TypeAlias,
                        node_line(&cap.node),
                        &name,
                        qualify_typedef_name(content, &cap.node, &name),
                    );
                }
            }
            continue;
        }

        if let Some(cap) = find_capture(m, idx_using_alias_name) {
            remember_qualified_name(
                &mut qualified,
                SymbolKind::TypeAlias,
                node_line(&cap.node),
                node_text(content, &cap.node),
                node_text_with_namespace(content, &cap.node),
            );
        }
    }

    Ok(qualified)
}

fn remember_qualified_name(
    qualified: &mut HashMap<(String, usize, String), String>,
    kind: SymbolKind,
    line: usize,
    bare_name: &str,
    qualified_name: String,
) {
    if bare_name != qualified_name {
        qualified.insert(
            (kind.as_str().to_string(), line, bare_name.to_string()),
            qualified_name,
        );
    }
}

fn enclosing_namespaces<'a>(content: &'a str, node: Node<'a>) -> Vec<&'a str> {
    let mut namespaces = Vec::new();
    let mut current = node.parent();

    if let Some(parent) = current {
        let is_current_namespace_name = parent.kind() == "namespace_definition"
            && parent
                .child_by_field_name("name")
                .map(|name_node| name_node == node)
                .unwrap_or(false);
        if is_current_namespace_name {
            current = parent.parent();
        }
    }

    while let Some(parent) = current {
        if parent.kind() == "namespace_definition" {
            if let Some(name_node) = parent.child_by_field_name("name") {
                let name = node_text(content, &name_node);
                if !name.is_empty() {
                    namespaces.push(name);
                }
            }
        }
        current = parent.parent();
    }

    namespaces.reverse();
    namespaces
}

fn node_text_with_namespace(content: &str, node: &Node) -> String {
    let mut parts = enclosing_namespaces(content, *node);
    parts.push(node_text(content, node));
    parts.join("::")
}

fn qualify_typedef_name(content: &str, type_def_node: &tree_sitter::Node, name: &str) -> String {
    let mut parts = enclosing_namespaces(content, *type_def_node);
    parts.push(name);
    parts.join("::")
}

fn qualify_enum_value(
    content: &str,
    enum_name_node: &tree_sitter::Node,
    enum_name: &str,
    value_name: &str,
) -> String {
    let enum_node = enum_name_node.parent();
    let enum_text = enum_node
        .as_ref()
        .map(|node| node_text(content, node))
        .unwrap_or("");

    let mut parts = enclosing_namespaces(content, *enum_name_node);
    // Scoped enums (`enum class` / `enum struct`) require EnumName::Value.
    if enum_text.contains("enum class") || enum_text.contains("enum struct") {
        parts.push(enum_name);
    }
    parts.push(value_name);
    parts.join("::")
}

/// Extract the name from a complex typedef declaration.
/// For `typedef void (*Callback)(int, int);`, the name "Callback" is nested inside
/// function_declarator -> parenthesized_declarator -> pointer_declarator -> type_identifier.
/// This function recursively walks the declarator to find the identifier.
fn extract_typedef_name(type_def_node: &tree_sitter::Node, content: &str) -> Option<String> {
    // Look for the "declarator" field on the type_definition node
    let declarator = type_def_node.child_by_field_name("declarator")?;
    find_identifier_in_declarator(&declarator, content)
}

/// Recursively search a declarator subtree for the first type_identifier or identifier
fn find_identifier_in_declarator(node: &tree_sitter::Node, content: &str) -> Option<String> {
    // If this node is a type_identifier or identifier, it's our name
    if node.kind() == "type_identifier" || node.kind() == "identifier" {
        return Some(node_text(content, node).to_string());
    }

    // Recurse into children
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if let Some(name) = find_identifier_in_declarator(&child, content) {
            return Some(name);
        }
    }
    None
}

/// Extract base class names from a class/struct specifier node.
/// Walks up to the parent (class_specifier or struct_specifier) and looks for base_class_clause.
fn extract_base_classes(content: &str, name_node: &tree_sitter::Node) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    if let Some(class_node) = name_node.parent() {
        let mut walker = class_node.walk();
        for child in class_node.children(&mut walker) {
            if child.kind() == "base_class_clause" {
                let mut inner_walker = child.walk();
                for base_child in child.children(&mut inner_walker) {
                    // Look for type_identifier or template_type nodes inside base_class_clause
                    if base_child.kind() == "type_identifier" {
                        let base_name = node_text(content, &base_child);
                        parents.push((base_name.to_string(), "extends".to_string()));
                    } else if base_child.kind() == "template_type" {
                        // template_type has a name child (type_identifier)
                        let mut tt_walker = base_child.walk();
                        for tt_child in base_child.children(&mut tt_walker) {
                            if tt_child.kind() == "type_identifier" {
                                let base_name = node_text(content, &tt_child);
                                parents.push((base_name.to_string(), "extends".to_string()));
                                break;
                            }
                        }
                    } else if base_child.kind() == "qualified_identifier" {
                        let base_name = node_text(content, &base_child);
                        parents.push((base_name.to_string(), "extends".to_string()));
                    } else if base_child.kind() == "access_specifier" {
                        // Skip access specifiers (public, private, protected)
                        continue;
                    }
                }
            }
        }
    }
    parents
}

/// Check if a line looks like a JNI function declaration
fn is_jni_function(line: &str) -> bool {
    line.contains("JNIEXPORT") && line.contains("JNICALL")
}

/// Extract the method name from a JNI function (last part after last underscore in Java_... name)
fn extract_jni_method_name(line: &str) -> Option<String> {
    // Find Java_... pattern in the line
    let java_start = line.find("Java_")?;
    let rest = &line[java_start..];
    // The JNI name ends at '(' or whitespace
    let end = rest
        .find(|c: char| c == '(' || c.is_whitespace())
        .unwrap_or(rest.len());
    let jni_name = &rest[..end];
    // Method name is after the last '_'
    let method = jni_name.rsplit('_').next()?;
    if method.is_empty() {
        None
    } else {
        Some(method.to_string())
    }
}

/// Check if name is a C++ reserved word
fn is_reserved_word(name: &str) -> bool {
    matches!(
        name,
        "if" | "else"
            | "while"
            | "for"
            | "do"
            | "switch"
            | "case"
            | "default"
            | "break"
            | "continue"
            | "return"
            | "goto"
            | "try"
            | "catch"
            | "throw"
            | "new"
            | "delete"
            | "this"
            | "sizeof"
            | "typeid"
            | "static_cast"
            | "dynamic_cast"
            | "const_cast"
            | "reinterpret_cast"
            | "nullptr"
            | "true"
            | "false"
            | "auto"
            | "register"
            | "static"
            | "extern"
            | "mutable"
            | "thread_local"
            | "inline"
            | "virtual"
            | "explicit"
            | "friend"
            | "constexpr"
            | "decltype"
            | "noexcept"
            | "override"
            | "final"
            | "public"
            | "private"
            | "protected"
            | "using"
            | "namespace"
            | "class"
            | "struct"
            | "union"
            | "enum"
            | "typedef"
            | "template"
            | "typename"
            | "concept"
            | "requires"
            | "co_await"
            | "co_return"
            | "co_yield"
            | "operator"
            | "main"
    )
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

    // --- Functions returning a pointer or a reference ---

    #[test]
    fn test_parse_pointer_and_reference_return_functions() {
        let content = r#"
GridMap* TerrainInfo::GetGrid(uint32 mapId, float x, float y, bool loadIfMissing /*= true*/)
{
    return nullptr;
}

Creature* Player::GetNPCIfCanInteractWith(ObjectGuid const& guid, NPCFlags npcFlags) const
{
    return nullptr;
}

PhaseShift const& PhasingHandler::GetEmptyPhaseShift()
{
    static PhaseShift const empty;
    return empty;
}

Player* FindPlayer(ObjectGuid const& guid)
{
    return nullptr;
}

char** SplitArgs(char const* line)
{
    return nullptr;
}

Color4& Color4::operator=(Color4 const& other)
{
    return *this;
}

template <class T>
T* MakeOne()
{
    return nullptr;
}

uint32 Plain()
{
    return 0;
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        for name in [
            "GetGrid",
            "GetNPCIfCanInteractWith",
            "GetEmptyPhaseShift",
            "FindPlayer",
            "SplitArgs",
            "operator=",
            "MakeOne",
            "Plain",
        ] {
            let found: Vec<_> = symbols.iter().filter(|s| s.name == name).collect();
            assert_eq!(
                found.len(),
                1,
                "Expected exactly one symbol named {}, got: {:?}",
                name,
                symbols
            );
            assert_eq!(
                found[0].kind,
                SymbolKind::Function,
                "{} should be a function",
                name
            );
        }
        let get_grid = symbols.iter().find(|s| s.name == "GetGrid").unwrap();
        assert!(
            get_grid.parents.iter().any(|(p, _)| p == "TerrainInfo"),
            "Expected GetGrid to belong to TerrainInfo, got: {:?}",
            get_grid.parents
        );
        let assign = symbols.iter().find(|s| s.name == "operator=").unwrap();
        assert!(
            assign.parents.iter().any(|(p, _)| p == "Color4"),
            "Expected operator= to belong to Color4, got: {:?}",
            assign.parents
        );
    }

    // --- Classes ---

    #[test]
    fn test_parse_class() {
        let content = r#"
class TJavaException {
public:
    TJavaException() {}
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Class && s.name == "TJavaException"),
            "Expected to find class TJavaException, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_class_with_inheritance() {
        let content = r#"
class TJniClass : public TJniReference {
public:
    TJniClass() {}
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.name == "TJniClass")
            .expect("TJniClass not found");
        assert_eq!(class.kind, SymbolKind::Class);
        assert!(
            class.parents.iter().any(|(p, _)| p == "TJniReference"),
            "Expected parent TJniReference, got: {:?}",
            class.parents
        );
    }

    #[test]
    fn test_parse_class_with_template_base() {
        let content = r#"
class TJniClass : public TJniReference<jclass> {
public:
    TJniClass() {}
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.name == "TJniClass")
            .expect("TJniClass not found");
        assert_eq!(class.kind, SymbolKind::Class);
        assert!(
            class.parents.iter().any(|(p, _)| p == "TJniReference"),
            "Expected parent TJniReference, got: {:?}",
            class.parents
        );
    }

    #[test]
    fn test_parse_struct() {
        let content = r#"
struct Point {
    int x;
    int y;
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Class && s.name == "Point"),
            "Expected to find struct Point as Class, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_template_class() {
        let content = r#"
template<class T>
class TJniReference : public TNonCopyable {
    T value_;
public:
    T Get() const { return value_; }
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Class && s.name == "TJniReference"),
            "Expected to find template class TJniReference, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_skip_forward_declaration() {
        let content = r#"
class Foo;
struct Bar;
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        // Forward declarations have no body, so should not be captured
        assert!(
            !symbols
                .iter()
                .any(|s| s.name == "Foo" && s.kind == SymbolKind::Class),
            "Forward declaration class Foo should be skipped, got: {:?}",
            symbols
        );
        assert!(
            !symbols
                .iter()
                .any(|s| s.name == "Bar" && s.kind == SymbolKind::Class),
            "Forward declaration struct Bar should be skipped, got: {:?}",
            symbols
        );
    }

    // --- Functions ---

    #[test]
    fn test_parse_function() {
        let content = r#"
void doSomething(int x) {
    return;
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "doSomething"),
            "Expected function doSomething, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_template_function() {
        let content = r#"
template<class Func>
auto jniWrapExceptions(JNIEnv* env, Func&& func) {
    try { return func(); }
    catch (...) { }
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "jniWrapExceptions"),
            "Expected template function jniWrapExceptions, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_method_definition() {
        let content = r#"
void MyClass::doWork(int x) {
    return;
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        let method = symbols
            .iter()
            .find(|s| s.name == "doWork")
            .expect("doWork not found");
        assert_eq!(method.kind, SymbolKind::Function);
        assert!(
            method
                .parents
                .iter()
                .any(|(p, k)| p == "MyClass" && k == "member"),
            "Expected parent MyClass with role member, got: {:?}",
            method.parents
        );
    }

    #[test]
    fn test_parse_destructor() {
        let content = r#"
MyClass::~MyClass() {
    cleanup();
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols.iter().any(|s| s.name == "~MyClass"
                && s.kind == SymbolKind::Function
                && s.parents
                    .iter()
                    .any(|(p, k)| p == "MyClass" && k == "member")),
            "Expected destructor ~MyClass with parent MyClass, got: {:?}",
            symbols
        );
    }

    // --- Namespaces ---

    #[test]
    fn test_parse_namespace() {
        let content = r#"
namespace NDirect {
    class Foo {};
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Package && s.name == "NDirect"),
            "Expected namespace NDirect, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_nested_namespace() {
        let content = r#"
namespace outer {
    namespace inner {
        void foo() {}
    }
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Package && s.name == "outer"),
            "Expected namespace outer, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Package && s.name == "inner"),
            "Expected namespace inner, got: {:?}",
            symbols
        );
    }

    // --- Enums ---

    #[test]
    fn test_parse_enum() {
        let content = r#"
enum Color {
    RED,
    GREEN,
    BLUE
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Enum && s.name == "Color"),
            "Expected enum Color, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "RED"),
            "Expected enum value RED, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "GREEN"),
            "Expected enum value GREEN, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "BLUE"),
            "Expected enum value BLUE, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_enum_class() {
        let content = r#"
enum class Status {
    Active,
    Inactive,
    Deleted
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Enum && s.name == "Status"),
            "Expected enum class Status, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Active"),
            "Expected enum class value Active, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Inactive"),
            "Expected enum class value Inactive, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Deleted"),
            "Expected enum class value Deleted, got: {:?}",
            symbols
        );
    }

    // --- Type aliases ---

    #[test]
    fn test_parse_typedef() {
        let content = r#"
typedef unsigned long ulong;
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::TypeAlias && s.name == "ulong"),
            "Expected typedef ulong, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_using_alias() {
        let content = r#"
using StringVec = std::vector<std::string>;
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::TypeAlias && s.name == "StringVec"),
            "Expected using alias StringVec, got: {:?}",
            symbols
        );
    }

    // --- Macros ---

    #[test]
    fn test_parse_function_macro() {
        let content = r#"
#define MAX(a, b) ((a) > (b) ? (a) : (b))
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "MAX"),
            "Expected macro MAX, got: {:?}",
            symbols
        );
    }

    // --- Includes ---

    #[test]
    fn test_parse_includes() {
        let content = r#"
#include <jni.h>
#include "util.h"
#include <util/generic/string.h>
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Import && s.name == "jni.h"),
            "Expected include jni.h, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Import && s.name == "util.h"),
            "Expected include util.h, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Import && s.name == "string.h"),
            "Expected include string.h (from util/generic/string.h), got: {:?}",
            symbols
        );
    }

    // --- Comments are ignored ---

    #[test]
    fn test_comments_ignored() {
        let content = r#"
// class FakeClass {};
class RealClass {
    int x;
};
/* void fakeFunc() {} */
void realFunc() {
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols.iter().any(|s| s.name == "RealClass"),
            "Expected RealClass"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "FakeClass"),
            "FakeClass should be ignored (in comment)"
        );
        assert!(
            symbols.iter().any(|s| s.name == "realFunc"),
            "Expected realFunc"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "fakeFunc"),
            "fakeFunc should be ignored (in comment)"
        );
    }

    // --- Complex scenarios ---

    #[test]
    fn test_parse_class_with_methods_and_namespace() {
        let content = r#"
namespace mylib {

class Widget {
public:
    void draw();
    int size() const;
};

void Widget::draw() {
}

int Widget::size() const {
    return 0;
}

} // namespace mylib
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Package && s.name == "mylib"),
            "Expected namespace mylib"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Class && s.name == "Widget"),
            "Expected class Widget"
        );
        assert!(
            symbols.iter().any(|s| s.name == "draw"
                && s.kind == SymbolKind::Function
                && s.parents.iter().any(|(p, _)| p == "Widget")),
            "Expected method draw with parent Widget"
        );
        assert!(
            symbols.iter().any(|s| s.name == "size"
                && s.kind == SymbolKind::Function
                && s.parents.iter().any(|(p, _)| p == "Widget")),
            "Expected method size with parent Widget"
        );
    }

    #[test]
    fn test_parse_namespaced_free_function_and_enum() {
        let content = r#"
namespace arcanum {
    enum class Mode {
        Fast,
    };

    Client makeClient() {
        return {};
    }
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Enum && s.name == "Mode"),
            "Expected enum Mode, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Fast"),
            "Expected enum class value Fast, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "makeClient"),
            "Expected function makeClient, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_multiple_base_classes() {
        let content = r#"
class MyClass : public Base1, public Base2 {
    int x;
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.name == "MyClass")
            .expect("MyClass not found");
        assert!(
            class.parents.iter().any(|(p, _)| p == "Base1"),
            "Expected parent Base1, got: {:?}",
            class.parents
        );
        assert!(
            class.parents.iter().any(|(p, _)| p == "Base2"),
            "Expected parent Base2, got: {:?}",
            class.parents
        );
    }

    #[test]
    fn test_parse_constexpr_function() {
        let content = r#"
constexpr int square(int x) {
    return x * x;
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "square"),
            "Expected constexpr function square, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_inline_function() {
        let content = r#"
inline void helper() {
    return;
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "helper"),
            "Expected inline function helper, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_static_function() {
        let content = r#"
static int counter() {
    static int c = 0;
    return ++c;
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "counter"),
            "Expected static function counter, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_template_struct() {
        let content = r#"
template<typename T>
struct Optional {
    T value;
    bool has_value;
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Class && s.name == "Optional"),
            "Expected template struct Optional, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_enum_with_type() {
        let content = r#"
enum class Color : uint8_t {
    Red = 0,
    Green = 1,
    Blue = 2
};
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Enum && s.name == "Color"),
            "Expected enum class Color with underlying type, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Red"),
            "Expected enum class value Red, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Green"),
            "Expected enum class value Green, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Constant && s.name == "Blue"),
            "Expected enum class value Blue, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_typedef_function_pointer() {
        let content = r#"
typedef void (*Callback)(int, int);
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::TypeAlias && s.name == "Callback"),
            "Expected typedef function pointer Callback, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_jni_extraction() {
        assert_eq!(
            extract_jni_method_name(
                "JNIEXPORT jobject JNICALL Java_com_example_TextProcessor_analyze"
            ),
            Some("analyze".to_string())
        );
    }

    #[test]
    fn test_reserved_words_filtered() {
        assert!(is_reserved_word("if"));
        assert!(is_reserved_word("class"));
        assert!(is_reserved_word("operator"));
        assert!(!is_reserved_word("doSomething"));
        assert!(!is_reserved_word("MyClass"));
    }

    #[test]
    fn test_parse_anonymous_namespace() {
        let content = r#"
namespace {
    void internal_func() {}
}
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();
        // Anonymous namespaces have no name, so no namespace symbol emitted
        assert!(
            !symbols.iter().any(|s| s.kind == SymbolKind::Package),
            "Anonymous namespace should not emit a Package symbol, got: {:?}",
            symbols
        );
        // But the function inside should still be captured
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == SymbolKind::Function && s.name == "internal_func"),
            "Expected function internal_func inside anonymous namespace"
        );
    }

    #[test]
    fn test_parse_complex_file() {
        let content = r#"
#include <iostream>
#include "myheader.h"

#define STRINGIFY(x) #x

namespace utils {

enum class LogLevel {
    Debug,
    Info,
    Warning,
    Error
};

class Logger {
public:
    void log(LogLevel level, const char* msg);
};

void Logger::log(LogLevel level, const char* msg) {
    std::cout << msg << std::endl;
}

template<typename T>
T clamp(T value, T lo, T hi) {
    return value < lo ? lo : value > hi ? hi : value;
}

typedef void (*LogCallback)(const char*);
using StringRef = const std::string&;

} // namespace utils
"#;
        let symbols = CPP_PARSER.parse_symbols(content).unwrap();

        // Includes
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Import && s.name == "iostream"));
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Import && s.name == "myheader.h"));

        // Macro
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Constant && s.name == "STRINGIFY"));

        // Namespace
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Package && s.name == "utils"));

        // Enum class
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Enum && s.name == "LogLevel"));
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Constant && s.name == "Debug"));
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Constant && s.name == "Info"));
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Constant && s.name == "Warning"));
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Constant && s.name == "Error"));

        // Class
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Class && s.name == "Logger"));

        // Method definition
        assert!(symbols.iter().any(|s| s.name == "log"
            && s.kind == SymbolKind::Function
            && s.parents.iter().any(|(p, _)| p == "Logger")));

        // Template function
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Function && s.name == "clamp"));

        // Typedef
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::TypeAlias && s.name == "LogCallback"));

        // Using alias
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::TypeAlias && s.name == "StringRef"));
    }
}
