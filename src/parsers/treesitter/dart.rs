//! Tree-sitter based Dart parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Node};

use super::{
    line_text, node_line, node_text, parse_tree, signature_line, text_end_line, walk_tree_preorder,
    LanguageParser, WalkControl,
};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static DART_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_dart::LANGUAGE.into());

pub static DART_PARSER: DartParser = DartParser;

pub struct DartParser;

/// Comments and string literals; the interpolations of a string are code.
static NON_CODE: super::NonCode = super::NonCode {
    language: &DART_LANGUAGE,
    prose: &["comment", "block_comment", "documentation_block_comment"],
    strings: &["string_literal"],
    code: &["template_substitution"],
    keep: super::keep_no_string,
};

impl LanguageParser for DartParser {
    fn non_code(&self) -> Option<&'static super::NonCode> {
        Some(&NON_CODE)
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &DART_LANGUAGE)?;
        let mut symbols = Vec::new();
        walk_node(&tree.root_node(), content, &mut symbols);
        Ok(symbols)
    }
}

fn walk_node(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let mut stack = vec![*node];

    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_or_export" => {
                extract_import(&node, content, symbols);
            }
            "class_declaration" => {
                extract_class(&node, content, symbols);
                walk_class_body(&node, content, symbols);
                continue;
            }
            "mixin_declaration" => {
                extract_mixin(&node, content, symbols);
                walk_class_body(&node, content, symbols);
                continue;
            }
            "extension_declaration" => {
                extract_extension(&node, content, symbols);
                walk_extension_body(&node, content, symbols);
                continue;
            }
            "extension_type_declaration" => {
                extract_extension_type(&node, content, symbols);
                walk_class_body(&node, content, symbols);
                continue;
            }
            "enum_declaration" => {
                extract_enum(&node, content, symbols);
                walk_class_body(&node, content, symbols);
                continue;
            }
            "type_alias" => {
                extract_typedef(&node, content, symbols);
                continue;
            }
            // Fall through to descend into the body — catches nested local functions.
            "function_declaration" | "external_function_declaration" => {
                if let Some(sig) = node.child_by_field_name("signature") {
                    extract_function_signature(&sig, content, symbols);
                }
            }
            "getter_declaration" | "external_getter_declaration" => {
                if let Some(sig) = node.child_by_field_name("signature") {
                    extract_getter(&sig, content, symbols);
                }
            }
            "setter_declaration" | "external_setter_declaration" => {
                if let Some(sig) = node.child_by_field_name("signature") {
                    extract_setter(&sig, content, symbols);
                }
            }
            "local_function_declaration" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "function_signature" {
                        extract_function_signature(&child, content, symbols);
                        break;
                    }
                }
            }
            "top_level_variable_declaration" | "external_variable_declaration" => {
                extract_top_level_variable_decl(&node, content, symbols);
                continue;
            }
            _ => {}
        }

        let mut cursor = node.walk();
        let mut children: Vec<Node> = node.children(&mut cursor).collect();
        children.reverse();
        stack.extend(children);
    }
}

/// Extract an `import`/`export` declaration as a single Import symbol.
///
/// Two AST shapes coexist in tree-sitter-dart 0.2.0:
///   library_import → import_specification → configurable_uri → uri
///   library_export → configurable_uri → uri        (no import_specification wrapper)
/// `find_descendant_by_kind` walks both uniformly.
fn extract_import(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    if let Some(uri_node) = find_descendant_by_kind(node, "uri") {
        let uri_text = node_text(content, &uri_node).trim().to_string();
        let path = uri_text
            .trim_start_matches('\'')
            .trim_end_matches('\'')
            .trim_start_matches('"')
            .trim_end_matches('"');
        let short_name = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".dart");
        symbols.push(ParsedSymbol {
            name: short_name.to_string(),
            kind: SymbolKind::Import,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(text_end_line(content, node)),
        });
    }
}

/// Extract names from nielsenko's top_level_variable_declaration.
/// Children: initialized_identifier_list, static_final_declaration_list,
/// or directly initialized_variable_definition for typed declarations.
///
/// Workaround: nielsenko grammar misparses `typedef Foo = Future<void> Function(...)`
/// as top_level_variable_declaration (the `<void>` confuses the parser as relational).
/// Detect this case by checking if first child is a `type` containing `typedef` keyword.
fn extract_top_level_variable_decl(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    // Scan any child (not just the first) for a `type` wrapper that holds the
    // literal keyword `typedef` — `@annotation` directives sit before it.
    let mut cursor = node.walk();
    let is_misparsed_typedef = node
        .children(&mut cursor)
        .any(|c| c.kind() == "type" && node_text(content, &c).trim() == "typedef");
    if is_misparsed_typedef {
        extract_misparsed_typedef(node, content, symbols);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "initialized_identifier_list" => extract_top_level_vars(&child, content, symbols),
            "static_final_declaration_list" => extract_top_level_consts(&child, content, symbols),
            "initialized_variable_definition" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(content, &name_node).to_string();
                    let line = node_line(&child);
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Property,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line: Some(text_end_line(content, &child)),
                    });
                }
            }
            // `external int x;` produces a bare identifier_list with no initializer.
            "identifier_list" => {
                let mut inner = child.walk();
                for id in child.children(&mut inner) {
                    if id.kind() == "identifier" {
                        let line = node_line(&id);
                        symbols.push(ParsedSymbol {
                            name: node_text(content, &id).to_string(),
                            kind: SymbolKind::Property,
                            line,
                            signature: signature_line(content, line),
                            parents: vec![],
                            end_line: Some(text_end_line(content, &id)),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Extract typedef name from a top_level_variable_declaration that nielsenko misparses.
/// AST shape:
///   top_level_variable_declaration
///     type (text "typedef")
///     initialized_identifier_list
///       initialized_identifier
///         identifier <NAME>   ← extract this
///         = ...
fn extract_misparsed_typedef(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "initialized_identifier_list" {
            let mut inner_cursor = child.walk();
            for inner in child.children(&mut inner_cursor) {
                if inner.kind() == "initialized_identifier" {
                    let mut id_cursor = inner.walk();
                    for grandchild in inner.children(&mut id_cursor) {
                        if grandchild.kind() == "identifier" {
                            let name = node_text(content, &grandchild).to_string();
                            if !name.is_empty() {
                                symbols.push(ParsedSymbol {
                                    name,
                                    kind: SymbolKind::TypeAlias,
                                    line,
                                    signature: sig.clone(),
                                    parents: vec![],
                                    end_line: Some(text_end_line(content, node)),
                                });
                            }
                            break;
                        }
                    }
                }
            }
            return;
        }
    }
}

/// Extract a class declaration as a Class or Interface symbol with its parents.
fn extract_class(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(content, &n).to_string(),
        None => return,
    };

    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    // Detect the `interface class` modifier by scanning the declaration prefix.
    let full_text = node_text(content, node);
    let decl_prefix = full_text.split('{').next().unwrap_or("");
    let kind =
        if decl_prefix.contains("interface class") || decl_prefix.contains("interface  class") {
            SymbolKind::Interface
        } else {
            SymbolKind::Class
        };

    let mut parents = Vec::new();
    // superclass field
    if let Some(superclass_node) = node.child_by_field_name("superclass") {
        extract_superclass_parents(&superclass_node, content, &mut parents);
    }
    // interfaces field
    if let Some(interfaces_node) = node.child_by_field_name("interfaces") {
        extract_interfaces_parents(&interfaces_node, content, &mut parents);
    }

    symbols.push(ParsedSymbol {
        name,
        kind,
        line,
        signature: sig,
        parents,
        end_line: Some(text_end_line(content, node)),
    });
}

/// Extract parents from a `superclass` field.
///
/// In tree-sitter-dart 0.2.0, `class C extends B<X>` lays out `superclass` as
///   extends, type 'B', type '<X>', ...
/// Only the first `type` sibling is the actual superclass; subsequent `type`
/// siblings wrap type arguments and must not be promoted to parents.
fn extract_superclass_parents(node: &Node, content: &str, parents: &mut Vec<(String, String)>) {
    let mut cursor = node.walk();
    let mut took_superclass = false;
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" if !took_superclass => {
                let name = node_text(content, &child).to_string();
                let base = name.split('<').next().unwrap_or(&name).trim().to_string();
                if !base.is_empty() {
                    parents.push((base, "extends".to_string()));
                    took_superclass = true;
                }
            }
            "type" if !took_superclass => {
                if let Some(name) = find_first_type_identifier(&child, content) {
                    let base = name.split('<').next().unwrap_or(&name).trim().to_string();
                    if !base.is_empty() {
                        parents.push((base, "extends".to_string()));
                        took_superclass = true;
                    }
                }
            }
            "mixins" => extract_mixins_parents(&child, content, parents),
            _ => {}
        }
    }
}

/// Extract parents from a `with` (mixins) clause.
fn extract_mixins_parents(node: &Node, content: &str, parents: &mut Vec<(String, String)>) {
    extract_type_names_from_node(node, content, parents, "with");
}

/// Extract parents from an `implements` (interfaces) clause.
fn extract_interfaces_parents(node: &Node, content: &str, parents: &mut Vec<(String, String)>) {
    extract_type_names_from_node(node, content, parents, "implements");
}

/// Collect every `type_identifier` descendant of `node` as a parent of `kind`.
fn extract_type_names_from_node(
    node: &Node,
    content: &str,
    parents: &mut Vec<(String, String)>,
    kind: &str,
) {
    walk_tree_preorder(node, |child| {
        if child.kind() == "type_arguments" {
            return WalkControl::SkipChildren;
        }
        if child.kind() == "type_identifier" {
            let name = node_text(content, &child).to_string();
            if !name.is_empty() {
                parents.push((name, kind.to_string()));
            }
        }
        WalkControl::Continue
    });
}

/// Extract a `mixin` declaration with its `on` and `implements` parents.
fn extract_mixin(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    let name = find_mixin_name(node, content);
    if name.is_empty() {
        return;
    }

    let mut parents = Vec::new();
    let node_text_full = node_text(content, node);
    let mut cursor = node.walk();
    let mut found_on = false;
    for child in node.children(&mut cursor) {
        match child.kind() {
            "on" => found_on = true,
            // nielsenko wraps each on-type in a `type` node containing the type_identifier.
            "type" if found_on => {
                if let Some(name) = find_first_type_identifier(&child, content) {
                    let base = name.split('<').next().unwrap_or(&name).trim().to_string();
                    if !base.is_empty() {
                        parents.push((base, "extends".to_string()));
                    }
                }
            }
            "interfaces" => extract_interfaces_parents(&child, content, &mut parents),
            _ => {}
        }
    }

    // Fallback: when the grammar doesn't expose `on` types as direct children,
    // parse them out of the source text.
    if parents.is_empty() && node_text_full.contains(" on ") {
        let on_part = node_text_full.split(" on ").nth(1).unwrap_or("");
        let on_types = on_part.split("implements").next().unwrap_or(on_part);
        let on_types = on_types.split('{').next().unwrap_or(on_types);
        for t in on_types.split(',') {
            let type_name = t.trim().split('<').next().unwrap_or("").trim();
            if !type_name.is_empty() {
                parents.push((type_name.to_string(), "extends".to_string()));
            }
        }
        if let Some(impl_part) = node_text_full.split("implements").nth(1) {
            let impl_part = impl_part.split('{').next().unwrap_or(impl_part);
            for t in impl_part.split(',') {
                let type_name = t.trim().split('<').next().unwrap_or("").trim();
                if !type_name.is_empty() {
                    parents.push((type_name.to_string(), "implements".to_string()));
                }
            }
        }
    }

    symbols.push(ParsedSymbol {
        name,
        kind: SymbolKind::Interface,
        line,
        signature: sig,
        parents,
        end_line: Some(text_end_line(content, node)),
    });
}

/// Return the `identifier` child of a `mixin_declaration` as a String.
fn find_mixin_name(node: &Node, content: &str) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return node_text(content, &child).to_string();
        }
    }
    String::new()
}

/// Extract an `extension` declaration with its on-type as the parent.
fn extract_extension(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(content, &n).to_string(),
        None => return,
    };

    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    let mut parents = Vec::new();

    // The on-type lives under the `class` field — nielsenko grammar quirk.
    if let Some(class_node) = node.child_by_field_name("class") {
        let on_type = if class_node.kind() == "type_identifier" {
            node_text(content, &class_node).to_string()
        } else {
            find_first_type_identifier(&class_node, content).unwrap_or_default()
        };
        let base = on_type
            .split('<')
            .next()
            .unwrap_or(&on_type)
            .trim()
            .to_string();
        if !base.is_empty() {
            parents.push((base, "extends".to_string()));
        }
    }

    symbols.push(ParsedSymbol {
        name,
        kind: SymbolKind::Object,
        line,
        signature: sig,
        parents,
        end_line: Some(text_end_line(content, node)),
    });
}

/// Extract an `extension type` declaration with its `implements` parents.
///
/// nielsenko exposes the name either as a dedicated `extension_type_name` node
/// (when type parameters are present) or as a plain `identifier`; both shapes
/// are handled. The `implements` clause is a flat sequence of children
/// (`implements` keyword followed by `type` nodes), not a named field.
fn extract_extension_type(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let name = match node.child_by_field_name("name") {
        Some(n) => {
            if n.kind() == "extension_type_name" {
                find_first_identifier(&n, content).unwrap_or_default()
            } else {
                node_text(content, &n).to_string()
            }
        }
        None => return,
    };
    if name.is_empty() {
        return;
    }

    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    // nielsenko does not expose `interfaces` as a field on extension_type_declaration —
    // the `implements` clause is laid out as a flat sequence of children: `implements`
    // keyword followed by one or more `type` nodes.
    let mut parents = Vec::new();
    let mut cursor = node.walk();
    let mut after_implements = false;
    for child in node.children(&mut cursor) {
        match child.kind() {
            "implements" => after_implements = true,
            "type" if after_implements => {
                if let Some(name) = find_first_type_identifier(&child, content) {
                    let base = name.split('<').next().unwrap_or(&name).trim().to_string();
                    if !base.is_empty() {
                        parents.push((base, "implements".to_string()));
                    }
                }
            }
            "class_body" => break,
            _ => {}
        }
    }

    symbols.push(ParsedSymbol {
        name,
        kind: SymbolKind::Class,
        line,
        signature: sig,
        parents,
        end_line: Some(text_end_line(content, node)),
    });
}

/// Extract an `enum` declaration with its `with` and `implements` parents.
fn extract_enum(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(content, &n).to_string(),
        None => return,
    };

    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    let mut parents = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "mixins" => extract_mixins_parents(&child, content, &mut parents),
            "interfaces" => extract_interfaces_parents(&child, content, &mut parents),
            _ => {}
        }
    }

    symbols.push(ParsedSymbol {
        name,
        kind: SymbolKind::Enum,
        line,
        signature: sig,
        parents,
        end_line: Some(text_end_line(content, node)),
    });
}

/// Extract a `typedef` as a TypeAlias. Handles both new-style `typedef Foo = X`
/// and old-style C-form `typedef Ret Name(args)` shapes.
fn extract_typedef(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    // Old-style C-form `typedef int OldStyle(int x)` lays out as
    //   typedef, type 'int', type_identifier 'OldStyle', formal_parameter_list
    // i.e. the return type appears BEFORE the typedef name. Prefer the
    // type_identifier that is a direct child (the name); fall back to deeper
    // search only for the new-style form `typedef Foo = ...` where the name
    // is the first type_identifier.
    let name = node
        .children(&mut node.walk())
        .find(|c| c.kind() == "type_identifier")
        .map(|c| node_text(content, &c).to_string())
        .or_else(|| find_first_type_identifier(node, content))
        .or_else(|| {
            let text = node_text(content, node);
            let after_typedef = text.strip_prefix("typedef")?.trim();
            let name_part = after_typedef.split(['=', '(', '<']).next()?;
            let tokens: Vec<&str> = name_part.split_whitespace().collect();
            tokens.last().map(|s| s.to_string())
        });

    if let Some(name) = name {
        if !name.is_empty() {
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::TypeAlias,
                line,
                signature: sig,
                parents: vec![],
                end_line: Some(text_end_line(content, node)),
            });
        }
    }
}

/// Extract a `function_signature` as a Function (or Property for `set`/`get` keywords).
fn extract_function_signature(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = node_text(content, &name_node).to_string();
        let line = node_line(node);
        let sig = line_text(content, line).trim().to_string();

        // Top-level `set foo(v) {}` and `get foo => …` parse as function_declaration —
        // the `set`/`get` keyword surfaces in the return_type slot. Re-classify them
        // as Property to keep semantic parity with class-level accessors.
        let kind = match node
            .child_by_field_name("return_type")
            .map(|n| node_text(content, &n).trim().to_string())
            .as_deref()
        {
            Some("set") | Some("get") => SymbolKind::Property,
            _ => SymbolKind::Function,
        };

        symbols.push(ParsedSymbol {
            name,
            kind,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(declaration_end_line(content, node)),
        });
    }
}

/// Extract a `getter_signature` as a Property.
fn extract_getter(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = node_text(content, &name_node).to_string();
        let line = node_line(node);
        let sig = line_text(content, line).trim().to_string();

        symbols.push(ParsedSymbol {
            name,
            kind: SymbolKind::Property,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(declaration_end_line(content, node)),
        });
    }
}

/// Extract a `setter_signature` as a Property.
fn extract_setter(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = node_text(content, &name_node).to_string();
        let line = node_line(node);
        let sig = line_text(content, line).trim().to_string();

        symbols.push(ParsedSymbol {
            name,
            kind: SymbolKind::Property,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(declaration_end_line(content, node)),
        });
    }
}

/// Walk a class, mixin, enum or extension-type body for member declarations.
fn walk_class_body(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let body = find_descendant_by_kind(node, "class_body")
        .or_else(|| find_descendant_by_kind(node, "enum_body"));

    if let Some(body) = body {
        walk_body_declarations(&body, content, symbols);
    }
}

/// Walk an `extension` body for member declarations.
fn walk_extension_body(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    if let Some(body) = node.child_by_field_name("body") {
        walk_body_declarations(&body, content, symbols);
    }
}

/// Dispatch each direct child of a body node to `walk_body_member`.
fn walk_body_declarations(body: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        walk_body_member(&child, content, symbols);
    }
}

/// nielsenko wraps inner declarations in `class_member`; method bodies are
/// walked recursively so local_function_declaration inside them is captured.
fn walk_body_member(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let mut stack = vec![*node];

    while let Some(node) = stack.pop() {
        match node.kind() {
            "class_member" => {
                let mut cursor = node.walk();
                let mut children: Vec<Node> = node.children(&mut cursor).collect();
                children.reverse();
                stack.extend(children);
            }
            "method_declaration" | "getter_declaration" | "setter_declaration" => {
                if let Some(sig) = node.child_by_field_name("signature") {
                    dispatch_member_node(&sig, content, symbols);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    walk_node(&body, content, symbols);
                }
            }
            "declaration" | "method_signature" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    dispatch_member_node(&child, content, symbols);
                }
            }
            _ => {
                if dispatch_member_node(&node, content, symbols) {
                    continue;
                }
                let mut cursor = node.walk();
                let mut children: Vec<Node> = node.children(&mut cursor).collect();
                children.reverse();
                stack.extend(children);
            }
        }
    }
}

/// Single source of truth for handling signature-shaped and field-shaped
/// children that may appear under `method_declaration > signature`,
/// `declaration`, or directly inside `class_body`. Returns true if the node
/// kind was recognised.
fn dispatch_member_node(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) -> bool {
    match node.kind() {
        "function_signature" => extract_function_signature(node, content, symbols),
        "getter_signature" => extract_getter(node, content, symbols),
        "setter_signature" => extract_setter(node, content, symbols),
        "constructor_signature" => extract_constructor(node, content, symbols),
        "factory_constructor_signature" => extract_factory_constructor(node, content, symbols),
        "constant_constructor_signature" => extract_const_constructor(node, content, symbols),
        "operator_signature" => extract_operator(node, content, symbols),
        // Instance fields: `class C { final int x = 1; }` lives under
        // declaration > initialized_identifier_list > initialized_identifier.
        "initialized_identifier_list" => extract_top_level_vars(node, content, symbols),
        "static_final_declaration_list" => extract_top_level_consts(node, content, symbols),
        // Wrappers — iterate their children and re-dispatch.
        "method_signature" | "declaration" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                dispatch_member_node(&child, content, symbols);
            }
        }
        _ => return false,
    }
    true
}

/// nielsenko AST for `bool operator ==(C other)`:
///   operator_signature
///     type [field=return_type]
///     operator                     (keyword)
///     binary_operator [field=operator] '=='
///     formal_parameter_list
/// The operator token lives in the `operator` field — its text is the symbol name
/// (e.g. `==`, `+`, `[]`).
fn extract_operator(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let op_text = node
        .child_by_field_name("operator")
        .map(|n| node_text(content, &n).trim().to_string())
        .unwrap_or_default();
    if op_text.is_empty() {
        return;
    }

    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();
    symbols.push(ParsedSymbol {
        name: format!("operator{op_text}"),
        kind: SymbolKind::Function,
        line,
        signature: sig,
        parents: vec![],
        end_line: Some(declaration_end_line(content, node)),
    });
}

/// Extract a `constructor_signature` as a Function (preserving any `.named` part).
fn extract_constructor(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    // child_by_field_name("name") returns only the class part — collect the
    // full `ClassName.namedPart` form manually.
    let name_text = collect_constructor_name(node, content);

    if !name_text.is_empty() {
        symbols.push(ParsedSymbol {
            name: name_text,
            kind: SymbolKind::Function,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(declaration_end_line(content, node)),
        });
    }
}

/// Build the full constructor name (e.g. `ClassName.named`) by joining all
/// identifier children before the parameter list.
fn collect_constructor_name(node: &Node, content: &str) -> String {
    let mut parts = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            parts.push(node_text(content, &child));
        }
        // Parameters may contain `this.foo` identifiers — stop before them.
        if child.kind() == "formal_parameter_list" {
            break;
        }
    }
    parts.join(".")
}

/// Extract a `factory_constructor_signature` as a Function.
fn extract_factory_constructor(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    let name = collect_constructor_name(node, content);

    if !name.is_empty() {
        symbols.push(ParsedSymbol {
            name,
            kind: SymbolKind::Function,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(declaration_end_line(content, node)),
        });
    }
}

/// Extract a `constant_constructor_signature` as a Function.
fn extract_const_constructor(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let line = node_line(node);
    let sig = line_text(content, line).trim().to_string();

    let name = collect_constructor_name(node, content);

    if !name.is_empty() {
        symbols.push(ParsedSymbol {
            name,
            kind: SymbolKind::Function,
            line,
            signature: sig,
            parents: vec![],
            end_line: Some(declaration_end_line(content, node)),
        });
    }
}

/// Extract names from an `initialized_identifier_list` as Property symbols
/// (top-level `final`/`var`/typed declarations and class-level instance fields).
fn extract_top_level_vars(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "initialized_identifier" {
            if let Some(id) = find_first_identifier(&child, content) {
                let line = node_line(&child);
                symbols.push(ParsedSymbol {
                    name: id,
                    kind: SymbolKind::Property,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line: Some(text_end_line(content, &child)),
                });
            }
        }
    }
}

/// Extract names from a `static_final_declaration_list` (multi-name `const`/`final`).
fn extract_top_level_consts(node: &Node, content: &str, symbols: &mut Vec<ParsedSymbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "static_final_declaration" {
            if let Some(id) = find_first_identifier(&child, content) {
                let line = node_line(&child);
                symbols.push(ParsedSymbol {
                    name: id,
                    kind: SymbolKind::Property,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line: Some(text_end_line(content, &child)),
                });
            }
        }
    }
}

/// Last line of the declaration a signature belongs to. The grammar keeps a
/// member's body beside its signature (`method_declaration` holds both, and a
/// `class_member` wraps that), so the range comes from the outermost wrapper
/// rather than from the signature itself.
fn declaration_end_line(content: &str, signature: &Node) -> usize {
    let mut declaration = *signature;
    while let Some(parent) = declaration.parent() {
        if !matches!(
            parent.kind(),
            "method_signature"
                | "method_declaration"
                | "declaration"
                | "class_member"
                | "function_declaration"
                | "local_function_declaration"
                | "getter_declaration"
                | "setter_declaration"
                | "external_function_declaration"
                | "external_getter_declaration"
                | "external_setter_declaration"
        ) {
            break;
        }
        declaration = parent;
    }
    text_end_line(content, &declaration)
}

/// Return the first `identifier` direct child of `node`.
fn find_first_identifier(node: &Node, content: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(content, &child).to_string());
        }
    }
    None
}

/// Return the first `type_identifier` descendant of `node` (DFS, pre-order).
fn find_first_type_identifier(node: &Node, content: &str) -> Option<String> {
    let mut found = None;
    walk_tree_preorder(node, |child| {
        if child.kind() == "type_identifier" {
            found = Some(node_text(content, &child).to_string());
            WalkControl::Stop
        } else {
            WalkControl::Continue
        }
    });
    found
}

/// Return the first descendant of `node` whose kind matches `kind` (DFS, pre-order).
fn find_descendant_by_kind<'a>(node: &Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut found = None;
    walk_tree_preorder(node, |child| {
        if child.kind() == kind {
            found = Some(child);
            WalkControl::Stop
        } else {
            WalkControl::Continue
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = "class MyWidget extends StatefulWidget {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "MyWidget").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "StatefulWidget" && k == "extends"),
            "Expected extends StatefulWidget, got: {:?}",
            cls.parents
        );
    }

    #[test]
    fn test_parse_abstract_class() {
        let content = "abstract class BaseService {\n  Future<void> init();\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "BaseService").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_sealed_class() {
        let content = "sealed class Result {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "Result")
            .unwrap_or_else(|| panic!("Should find sealed class Result, got: {symbols:?}"));
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_abstract_interface_class() {
        let content = "abstract interface class AppScope {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "AppScope").unwrap();
        assert_eq!(
            cls.kind,
            SymbolKind::Interface,
            "abstract interface class should be Interface, got: {:?}",
            cls.kind
        );
    }

    #[test]
    fn test_parse_class_with_parents() {
        let content =
            "class ApiService extends BaseService with LoggerMixin implements Disposable {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "ApiService" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "BaseService" && k == "extends"),
            "Expected extends BaseService, got: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "LoggerMixin" && k == "with"),
            "Expected with LoggerMixin, got: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "Disposable" && k == "implements"),
            "Expected implements Disposable, got: {:?}",
            cls.parents
        );
    }

    #[test]
    fn test_parse_mixin() {
        let content = "mixin LoggerMixin on Object {\n  void log(String msg) {}\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let m = symbols.iter().find(|s| s.name == "LoggerMixin").unwrap();
        assert_eq!(m.kind, SymbolKind::Interface);
        assert!(
            m.parents
                .iter()
                .any(|(p, k)| p == "Object" && k == "extends"),
            "Expected extends Object, got: {:?}",
            m.parents
        );
    }

    #[test]
    fn test_parse_mixin_with_implements() {
        let content = "mixin _PublicAppScopeImpl on _AppScopeDeps implements AppScope {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let m = symbols
            .iter()
            .find(|s| s.name == "_PublicAppScopeImpl")
            .unwrap();
        assert_eq!(m.kind, SymbolKind::Interface);
        assert!(
            m.parents
                .iter()
                .any(|(p, k)| p == "_AppScopeDeps" && k == "extends"),
            "should have _AppScopeDeps as extends parent, got: {:?}",
            m.parents
        );
        assert!(
            m.parents
                .iter()
                .any(|(p, k)| p == "AppScope" && k == "implements"),
            "should have AppScope as implements parent, got: {:?}",
            m.parents
        );
    }

    #[test]
    fn test_parse_extension() {
        let content = "extension DateTimeX on DateTime {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let ext = symbols.iter().find(|s| s.name == "DateTimeX").unwrap();
        assert_eq!(ext.kind, SymbolKind::Object);
        assert!(
            ext.parents
                .iter()
                .any(|(p, k)| p == "DateTime" && k == "extends"),
            "Expected extends DateTime, got: {:?}",
            ext.parents
        );
    }

    #[test]
    fn test_parse_extension_type() {
        let content = "extension type UserId(int id) implements int {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let et = symbols
            .iter()
            .find(|s| s.name == "UserId")
            .unwrap_or_else(|| panic!("Should find extension type UserId, got: {symbols:?}"));
        assert_eq!(et.kind, SymbolKind::Class);
        assert!(
            et.parents.iter().any(|(p, _)| p == "int"),
            "Expected implements int, got: {:?}",
            et.parents
        );
    }

    #[test]
    fn test_parse_enum() {
        let content = "enum Status {\n  loading,\n  success,\n  error,\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let e = symbols.iter().find(|s| s.name == "Status").unwrap();
        assert_eq!(e.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_enum_with_parents() {
        let content =
            "enum EnhancedEnum with Mixin implements Interface {\n  value1,\n  value2;\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let e = symbols.iter().find(|s| s.name == "EnhancedEnum").unwrap();
        assert_eq!(e.kind, SymbolKind::Enum);
        assert!(
            e.parents.iter().any(|(p, k)| p == "Mixin" && k == "with"),
            "Expected with Mixin, got: {:?}",
            e.parents
        );
        assert!(
            e.parents
                .iter()
                .any(|(p, k)| p == "Interface" && k == "implements"),
            "Expected implements Interface, got: {:?}",
            e.parents
        );
    }

    #[test]
    fn test_parse_typedef() {
        let content = "typedef JsonMap = Map<String, dynamic>;\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let td = symbols.iter().find(|s| s.name == "JsonMap").unwrap();
        assert_eq!(td.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_typedef_callback() {
        let content = "typedef VoidCallback = void Function();\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let td = symbols.iter().find(|s| s.name == "VoidCallback").unwrap();
        assert_eq!(td.kind, SymbolKind::TypeAlias);
    }

    // Grammar misparses `typedef Foo = Future<void> Function(...)` as a variable
    // declaration; extract_misparsed_typedef recovers the name from the
    // initialized_identifier_list shape.
    #[test]
    fn test_parse_misparsed_typedef() {
        let content = "typedef AsyncCallback = Future<void> Function(int x);\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let td = symbols
            .iter()
            .find(|s| s.name == "AsyncCallback")
            .unwrap_or_else(|| panic!("Should find AsyncCallback, got: {symbols:?}"));
        assert_eq!(td.kind, SymbolKind::TypeAlias);
    }

    // Annotations precede the misparsed `typedef`; the detection must scan all
    // children, not just child(0).
    #[test]
    fn test_parse_annotated_misparsed_typedef() {
        let content = "@deprecated\ntypedef DeprCallback = Future<void> Function();\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let td = symbols
            .iter()
            .find(|s| s.name == "DeprCallback")
            .unwrap_or_else(|| panic!("Should find DeprCallback, got: {symbols:?}"));
        assert_eq!(td.kind, SymbolKind::TypeAlias);
    }

    // Old C-form typedef puts the return type before the name; ensure the name
    // is extracted, not the return type.
    #[test]
    fn test_parse_old_style_typedef() {
        let content = "typedef int OldStyle(int x);\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let td = symbols
            .iter()
            .find(|s| s.name == "OldStyle")
            .unwrap_or_else(|| panic!("Should find OldStyle, got: {symbols:?}"));
        assert_eq!(td.kind, SymbolKind::TypeAlias);
        assert!(
            symbols.iter().all(|s| s.name != "int"),
            "Must not record return type as the typedef name"
        );
    }

    // `class C extends B<X>` must yield exactly one parent, not B and X.
    #[test]
    fn test_parse_class_extends_generic() {
        let content = "class C extends B<X> {}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "C")
            .unwrap_or_else(|| panic!("Should find class C, got: {symbols:?}"));
        let extends: Vec<_> = cls
            .parents
            .iter()
            .filter(|(_, kind)| kind == "extends")
            .collect();
        assert_eq!(
            extends.len(),
            1,
            "Expected one extends parent, got: {:?}",
            cls.parents
        );
        assert_eq!(extends[0].0, "B");
    }

    // `mixin M on Base implements I {}` — both relationships must be captured.
    #[test]
    fn test_parse_mixin_with_on_and_implements() {
        let content = "mixin M on Base implements I {}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let m = symbols
            .iter()
            .find(|s| s.name == "M")
            .unwrap_or_else(|| panic!("Should find mixin M, got: {symbols:?}"));
        assert!(
            m.parents.iter().any(|(p, k)| p == "Base" && k == "extends"),
            "Expected on Base, got: {:?}",
            m.parents
        );
        assert!(
            m.parents.iter().any(|(p, k)| p == "I" && k == "implements"),
            "Expected implements I, got: {:?}",
            m.parents
        );
    }

    // Extension type `implements` is a flat sequence in nielsenko (no `interfaces` field).
    #[test]
    fn test_parse_extension_type_implements() {
        let content = "extension type UserId(int id) implements int {}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let et = symbols
            .iter()
            .find(|s| s.name == "UserId")
            .unwrap_or_else(|| panic!("Should find UserId, got: {symbols:?}"));
        assert!(
            et.parents
                .iter()
                .any(|(p, k)| p == "int" && k == "implements"),
            "Expected implements int, got: {:?}",
            et.parents
        );
    }

    // `external int x;` produces a bare `identifier_list` instead of the usual
    // `initialized_identifier_list`.
    #[test]
    fn test_parse_external_variable() {
        let content = "external int x;\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let x = symbols
            .iter()
            .find(|s| s.name == "x")
            .unwrap_or_else(|| panic!("Should find external variable x, got: {symbols:?}"));
        assert_eq!(x.kind, SymbolKind::Property);
    }

    #[test]
    fn test_parse_operator_overload() {
        let content = "class C { bool operator ==(C other) => false; }\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "operator==" && s.kind == SymbolKind::Function),
            "Should find operator==, got: {symbols:?}"
        );
    }

    // Class-level fields (`int x = 1;`) live under
    // class_member > declaration > initialized_identifier_list.
    #[test]
    fn test_parse_class_instance_fields() {
        let content = "class C { final int x = 1; int y = 0; }\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        for field in ["x", "y"] {
            let f = symbols
                .iter()
                .find(|s| s.name == field)
                .unwrap_or_else(|| panic!("Should find field {field}, got: {symbols:?}"));
            assert_eq!(f.kind, SymbolKind::Property);
        }
    }

    // Top-level `set foo(...) {}` parses as function_declaration; ensure it's
    // reclassified as Property.
    #[test]
    fn test_parse_top_level_setter_only() {
        let content = "set logLevel(int v) {}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let s = symbols
            .iter()
            .find(|s| s.name == "logLevel")
            .unwrap_or_else(|| panic!("Should find logLevel, got: {symbols:?}"));
        assert_eq!(s.kind, SymbolKind::Property);
    }

    // Nested `void inner() {}` inside a method body must be picked up via the
    // body recursion in walk_body_member.
    #[test]
    fn test_parse_local_function_in_method_body() {
        let content = "class Foo {\n  void m() {\n    void inner() {}\n    inner();\n  }\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "inner" && s.kind == SymbolKind::Function),
            "Should find nested local function inner, got: {symbols:?}"
        );
    }

    // Export form without import_specification — extract_import must
    // walk into the library_export shape.
    #[test]
    fn test_parse_export_with_show_clause() {
        let content = "export 'src/foo.dart' show Bar, Baz;\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "foo" && s.kind == SymbolKind::Import),
            "Should find export of foo, got: {symbols:?}"
        );
    }

    #[test]
    fn test_parse_function() {
        let content = "void main() {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "main" && s.kind == SymbolKind::Function),
            "Should find main function, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_async_function() {
        let content = "Future<int> fetchData() async {\n  return 0;\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "fetchData" && s.kind == SymbolKind::Function),
            "Should find fetchData function, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_arrow_function() {
        let content = "String formatName(String first, String last) => '$first $last';\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "formatName" && s.kind == SymbolKind::Function),
            "Should find formatName function, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_getter_setter() {
        let content = r#"class Foo {
  int get count => _count;
  set count(int value) {
    _count = value;
  }
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let getters: Vec<_> = symbols
            .iter()
            .filter(|s| s.name == "count" && s.kind == SymbolKind::Property)
            .collect();
        assert!(
            getters.len() >= 1,
            "should find getter 'count', got: {:?}",
            symbols
        );
        let setters: Vec<_> = symbols
            .iter()
            .filter(|s| {
                s.name == "count" && s.kind == SymbolKind::Property && s.signature.contains("set ")
            })
            .collect();
        assert!(
            setters.len() >= 1,
            "should find setter 'count', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_constructor() {
        let content = r#"class MyService {
  MyService(this._dep);
  MyService.fromJson(Map<String, dynamic> json) {}
  factory MyService.create() => MyService(Dep());
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "MyService" && s.kind == SymbolKind::Class),
            "Should find class MyService, got: {:?}",
            symbols
        );
        // Named constructors
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "MyService.fromJson" && s.kind == SymbolKind::Function),
            "Should find MyService.fromJson constructor, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "MyService.create" && s.kind == SymbolKind::Function),
            "Should find MyService.create factory constructor, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_import() {
        let content = "import 'package:flutter/material.dart';\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "material" && s.kind == SymbolKind::Import),
            "Should find import 'material', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_export() {
        let content = "export 'src/my_widget.dart';\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "my_widget" && s.kind == SymbolKind::Import),
            "Should find export 'my_widget', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_dart_async_import() {
        let content = "import 'dart:async';\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "dart:async" && s.kind == SymbolKind::Import),
            "Should find import 'dart:async', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_property() {
        let content = "final String appName = 'MyApp';\nconst int maxRetries = 3;\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "appName" && s.kind == SymbolKind::Property),
            "Should find property appName, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "maxRetries" && s.kind == SymbolKind::Property),
            "Should find property maxRetries, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_comments_ignored() {
        let content = r#"
// class FakeClass {
/* class AnotherFake { */
class RealClass {
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            !symbols.iter().any(|s| s.name == "FakeClass"),
            "Should not find FakeClass in comments"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "AnotherFake"),
            "Should not find AnotherFake in comments"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "RealClass" && s.kind == SymbolKind::Class),
            "Should find RealClass, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_method_inside_class() {
        let content = r#"class ApiService {
  Future<void> init() async {}
  void doSomething() {}
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "init" && s.kind == SymbolKind::Function),
            "Should find method init, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "doSomething" && s.kind == SymbolKind::Function),
            "Should find method doSomething, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_method_inside_extension() {
        let content = r#"extension ApiServiceX on ApiService {
  void ping() {}
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "ApiServiceX" && s.kind == SymbolKind::Object),
            "Should find extension ApiServiceX, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "ping" && s.kind == SymbolKind::Function),
            "Should find method ping, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_method_inside_mixin() {
        let content = r#"mixin LoggerMixin on Object {
  void log(String msg) {}
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "LoggerMixin" && s.kind == SymbolKind::Interface),
            "Should find mixin LoggerMixin, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "log" && s.kind == SymbolKind::Function),
            "Should find method log, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_full_dart_file() {
        let content = r#"
import 'package:flutter/material.dart';
import 'dart:async';

typedef JsonMap = Map<String, dynamic>;

const String appVersion = '1.0.0';

mixin LoggerMixin on Object {
  void log(String msg) {}
}

abstract class BaseService {
  Future<void> init();
}

class ApiService extends BaseService with LoggerMixin implements Disposable {
  final String baseUrl;

  ApiService(this.baseUrl);

  ApiService.withDefault() : baseUrl = 'https://api.example.com';

  factory ApiService.create() => ApiService.withDefault();

  Future<void> init() async {}

  String get endpoint => '$baseUrl/v1';

  set timeout(int value) {}
}

extension ApiServiceX on ApiService {
  void ping() {}
}

enum Status {
  loading,
  success,
  error,
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();

        // Imports
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "material" && s.kind == SymbolKind::Import),
            "Should find import 'material', got: {:?}",
            symbols
        );

        // Typedef
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "JsonMap" && s.kind == SymbolKind::TypeAlias),
            "Should find typedef JsonMap, got: {:?}",
            symbols
        );

        // Property
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "appVersion" && s.kind == SymbolKind::Property),
            "Should find property appVersion, got: {:?}",
            symbols
        );

        // Mixin
        let mixin = symbols.iter().find(|s| s.name == "LoggerMixin").unwrap();
        assert_eq!(mixin.kind, SymbolKind::Interface);

        // Abstract class
        let base = symbols.iter().find(|s| s.name == "BaseService").unwrap();
        assert_eq!(base.kind, SymbolKind::Class);

        // Class with full inheritance
        let api = symbols
            .iter()
            .find(|s| s.name == "ApiService" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(
            api.parents
                .iter()
                .any(|(p, k)| p == "BaseService" && k == "extends"),
            "Expected extends BaseService, got: {:?}",
            api.parents
        );
        assert!(
            api.parents
                .iter()
                .any(|(p, k)| p == "LoggerMixin" && k == "with"),
            "Expected with LoggerMixin, got: {:?}",
            api.parents
        );
        assert!(
            api.parents
                .iter()
                .any(|(p, k)| p == "Disposable" && k == "implements"),
            "Expected implements Disposable, got: {:?}",
            api.parents
        );

        // Constructors
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "ApiService.withDefault" && s.kind == SymbolKind::Function),
            "Should find constructor ApiService.withDefault, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "ApiService.create" && s.kind == SymbolKind::Function),
            "Should find factory ApiService.create, got: {:?}",
            symbols
        );

        // Getter/Setter
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "endpoint" && s.kind == SymbolKind::Property),
            "Should find getter endpoint, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "timeout" && s.kind == SymbolKind::Property),
            "Should find setter timeout, got: {:?}",
            symbols
        );

        // Extension
        let ext = symbols.iter().find(|s| s.name == "ApiServiceX").unwrap();
        assert_eq!(ext.kind, SymbolKind::Object);
        assert!(
            ext.parents
                .iter()
                .any(|(p, k)| p == "ApiService" && k == "extends"),
            "Expected extends ApiService, got: {:?}",
            ext.parents
        );

        // Enum
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum),
            "Should find enum Status, got: {:?}",
            symbols
        );

        // Function inside class
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "init" && s.kind == SymbolKind::Function),
            "Should find method init, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_class_with_generics() {
        let content = "class Repository<T extends Model> implements BaseRepo<T> {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "Repository" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "BaseRepo" && k == "implements"),
            "Expected implements BaseRepo, got: {:?}",
            cls.parents
        );
    }

    #[test]
    fn test_parse_base_class() {
        let content = "base class BaseModel {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "BaseModel")
            .unwrap_or_else(|| panic!("Should find base class BaseModel, got: {symbols:?}"));
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_final_class() {
        let content = "final class FinalModel {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "FinalModel")
            .unwrap_or_else(|| panic!("Should find final class FinalModel, got: {symbols:?}"));
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_mixin_class() {
        let content = "mixin class MixinClass {\n}\n";
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "MixinClass")
            .unwrap_or_else(|| panic!("Should find mixin class MixinClass, got: {symbols:?}"));
        assert_eq!(cls.kind, SymbolKind::Class);
    }

    #[test]
    fn test_parse_multiple_imports() {
        let content = r#"
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
export 'src/utils.dart';
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "material" && s.kind == SymbolKind::Import),
            "Should find material import, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "provider" && s.kind == SymbolKind::Import),
            "Should find provider import, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "utils" && s.kind == SymbolKind::Import),
            "Should find utils export, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_class_multiline() {
        let content = r#"class _AppScopeContainer extends AppScopeContainer
    with _AppScopeDeps, _AppScopeInitializeQueue, _PublicAppScopeImpl {
}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "_AppScopeContainer" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "AppScopeContainer" && k == "extends"),
            "should have AppScopeContainer as extends, got: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "_AppScopeDeps" && k == "with"),
            "should have _AppScopeDeps as with, got: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "_AppScopeInitializeQueue" && k == "with"),
            "should have _AppScopeInitializeQueue as with, got: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "_PublicAppScopeImpl" && k == "with"),
            "should have _PublicAppScopeImpl as with, got: {:?}",
            cls.parents
        );
    }

    #[test]
    fn test_parse_top_level_getter_setter() {
        let content = r#"
String get appName => 'MyApp';
set appName(String value) {}
"#;
        let symbols = DART_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "appName" && s.kind == SymbolKind::Property),
            "Should find top-level getter appName, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_deeply_nested_blocks_without_stack_overflow() {
        let depth = 12_000;
        let mut content = String::from("void outer() {\n");
        for _ in 0..depth {
            content.push_str("{\n");
        }
        content.push_str("void inner() {}\n");
        for _ in 0..depth {
            content.push_str("}\n");
        }
        content.push_str("}\n");

        let symbols = DART_PARSER.parse_symbols(&content).unwrap();

        assert!(symbols
            .iter()
            .any(|s| s.name == "outer" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "inner" && s.kind == SymbolKind::Function));
    }
}
