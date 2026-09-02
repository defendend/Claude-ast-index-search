//! Tree-sitter based Kotlin/Java parser

use anyhow::{anyhow, Result};
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator, Tree};

use super::{
    line_text, node_line, node_text, parse_tree, walk_tree_preorder, LanguageParser, WalkControl,
};
use crate::db::SymbolKind;
use crate::parsers::{
    extract_references_for_lang, truncate_context, FileType, ParsedRef, ParsedSymbol,
};

static KT_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_kotlin_ng::LANGUAGE.into());

static KT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&KT_LANGUAGE, include_str!("queries/kotlin.scm"))
        .expect("Failed to compile Kotlin tree-sitter query")
});

pub static KOTLIN_PARSER: KotlinParser = KotlinParser;

pub struct KotlinParser;

impl LanguageParser for KotlinParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_kotlin_tree(content)?;
        let mut symbols = Vec::new();
        let query = &*KT_QUERY;
        let mut cursor = QueryCursor::new();

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_class_name = idx("class_name");
        let idx_class_decl = idx("class_decl");
        let idx_object_name = idx("object_name");
        let idx_object_decl = idx("object_decl");
        let idx_func_name = idx("func_name");
        let idx_property_name = idx("property_name");
        let idx_typealias_name = idx("typealias_name");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Class or Interface declaration
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                let decl_cap = find_capture(m, idx_class_decl);
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                if let Some(decl) = decl_cap {
                    let decl_node = &decl.node;

                    // Determine if this is an interface or class
                    let is_interface = has_keyword(decl_node, content, "interface");

                    // Check modifiers for enum, sealed, data, etc.
                    let has_enum_modifier = has_class_modifier(decl_node, content, "enum");

                    let kind = if is_interface {
                        SymbolKind::Interface
                    } else if has_enum_modifier {
                        // enum class maps to Class (not Enum), matching regex parser
                        SymbolKind::Class
                    } else {
                        SymbolKind::Class
                    };

                    // Parse inheritance from delegation_specifiers
                    let parents = if is_interface {
                        // Interface parents are always "extends"
                        parse_delegation_specifiers_for_interface(decl_node, content)
                    } else {
                        parse_delegation_specifiers(decl_node, content)
                    };

                    symbols.push(ParsedSymbol {
                        name: name.to_string(),
                        kind,
                        line,
                        signature: line_text(content, line).trim().to_string(),
                        parents,
                    });
                }
                continue;
            }

            // Object declaration
            if let Some(name_cap) = find_capture(m, idx_object_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                let parents = if let Some(decl) = find_capture(m, idx_object_decl) {
                    parse_delegation_specifiers(&decl.node, content)
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

            // Function declaration
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

            // Property declaration (val/var)
            if let Some(cap) = find_capture(m, idx_property_name) {
                if !is_indexable_property_name(&cap.node) {
                    continue;
                }
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
            if let Some(cap) = find_capture(m, idx_typealias_name) {
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
        }

        Ok(symbols)
    }

    fn extract_refs_for_lang(
        &self,
        content: &str,
        _defined: &[ParsedSymbol],
        _file_type: FileType,
    ) -> Result<Vec<ParsedRef>> {
        let tree = parse_kotlin_tree(content)?;
        let masked = mask_non_reference_ranges(content, tree.root_node());
        let mut refs = extract_references_for_lang(&masked, &[], Some(FileType::Kotlin))?;

        // Reference matching happens against the masked, byte-for-byte aligned
        // source, but users should see the original source line as context.
        let original_lines: Vec<&str> = content.lines().collect();
        for reference in &mut refs {
            if let Some(line) = original_lines.get(reference.line.saturating_sub(1)) {
                reference.context = truncate_context(line.trim());
            }
        }

        Ok(refs)
    }
}

/// The upstream grammar currently treats `suspend { ... }` as a type modifier
/// and can recover by wrapping the entire containing declaration in one ERROR
/// node. Reparse a byte-aligned copy where only that unambiguous stdlib-call
/// token is made into an ordinary identifier. Keeping byte lengths identical
/// means query captures still point into the original source.
fn parse_kotlin_tree(content: &str) -> Result<Tree> {
    let tree = parse_tree(content, &KT_LANGUAGE)?;
    if !tree.root_node().has_error() {
        return Ok(tree);
    }

    let mut suspend_lambda_ranges = Vec::new();
    walk_tree_preorder(&tree.root_node(), |node| {
        if node.kind() == "type_modifiers"
            && node_text(content, &node).trim() == "suspend"
            && next_non_whitespace_is(content, node.end_byte(), b'{')
        {
            suspend_lambda_ranges.push(node.byte_range());
        }
        WalkControl::Continue
    });

    if suspend_lambda_ranges.is_empty() {
        return Ok(tree);
    }

    let mut recovered = content.as_bytes().to_vec();
    for range in suspend_lambda_ranges {
        // `susp_nd` is a valid identifier with the same byte length.
        recovered[range.start + 4] = b'_';
    }
    let recovered = std::str::from_utf8(&recovered).map_err(|error| {
        anyhow!("Kotlin suspend-lambda recovery produced invalid UTF-8: {error}")
    })?;
    let recovered_tree = parse_tree(recovered, &KT_LANGUAGE)?;
    if recovered_tree.root_node().has_error() {
        return Err(anyhow!(
            "Kotlin parser could not recover from a suspend-lambda syntax error"
        ));
    }

    Ok(recovered_tree)
}

fn next_non_whitespace_is(content: &str, offset: usize, expected: u8) -> bool {
    content.as_bytes()[offset..]
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == expected)
}

fn is_indexable_property_name(name: &Node<'_>) -> bool {
    let Some(variable) = name.parent() else {
        return false;
    };
    let Some(property) = variable
        .parent()
        .filter(|node| node.kind() == "property_declaration")
    else {
        return false;
    };

    property.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "source_file" | "class_body" | "enum_class_body"
        )
    })
}

/// Replace non-code and declaration-name bytes with spaces while preserving
/// line/byte offsets. String interpolation nodes are copied back verbatim so
/// executable `${...}` and `$name` expressions remain visible to extraction.
fn mask_non_reference_ranges(content: &str, root: Node<'_>) -> String {
    let original = content.as_bytes();
    let mut masked = original.to_vec();

    walk_tree_preorder(&root, |node| match node.kind() {
        "line_comment" | "block_comment" | "character_literal" | "package_header" | "import" => {
            mask_range(&mut masked, node.byte_range());
            WalkControl::SkipChildren
        }
        "string_literal" | "multiline_string_literal" => {
            mask_range(&mut masked, node.byte_range());
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "interpolation" {
                    masked[child.byte_range()].copy_from_slice(&original[child.byte_range()]);
                }
            }
            // Keep walking interpolation descendants: nested strings/comments
            // inside `${...}` still need their own masking pass.
            WalkControl::Continue
        }
        "identifier" if is_declaration_identifier(&node) => {
            mask_range(&mut masked, node.byte_range());
            WalkControl::SkipChildren
        }
        _ => WalkControl::Continue,
    });

    // Masking only replaces bytes with ASCII spaces, so valid UTF-8 remains valid.
    String::from_utf8(masked).expect("Kotlin reference mask must preserve UTF-8")
}

fn mask_range(bytes: &mut [u8], range: std::ops::Range<usize>) {
    for byte in &mut bytes[range] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

fn is_declaration_identifier(node: &Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };

    match parent.kind() {
        "class_declaration" | "object_declaration" | "function_declaration" => parent
            .child_by_field_name("name")
            .is_some_and(|name| name.id() == node.id()),
        "type_alias" => parent
            .child_by_field_name("type")
            .is_some_and(|name| name.id() == node.id()),
        // These grammar nodes keep the declared name as a direct identifier;
        // identifiers inside their nested type nodes remain references.
        "variable_declaration" | "parameter" | "class_parameter" => true,
        _ => false,
    }
}

/// Check if a class_declaration node contains a specific keyword (e.g., "interface", "class")
/// by looking at its anonymous children (the keyword tokens).
fn has_keyword(node: &tree_sitter::Node, content: &str, keyword: &str) -> bool {
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        // Anonymous children are keywords
        if !child.is_named() && node_text(content, &child) == keyword {
            return true;
        }
    }
    false
}

/// Check if a class_declaration has a specific class_modifier (e.g., "enum", "sealed", "data")
fn has_class_modifier(node: &tree_sitter::Node, content: &str, modifier: &str) -> bool {
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if child.kind() == "modifiers" {
            let mut mod_walker = child.walk();
            for mod_child in child.children(&mut mod_walker) {
                if mod_child.kind() == "class_modifier"
                    && node_text(content, &mod_child) == modifier
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Parse delegation_specifiers from a class/object declaration node.
/// Returns parent list with (name, inherit_kind) where:
/// - constructor_invocation (has parentheses) -> "extends"
/// - plain type (no parentheses) -> "implements"
fn parse_delegation_specifiers(
    decl_node: &tree_sitter::Node,
    content: &str,
) -> Vec<(String, String)> {
    let mut parents = Vec::new();

    let mut walker = decl_node.walk();
    for child in decl_node.children(&mut walker) {
        if child.kind() == "delegation_specifiers" {
            let mut ds_walker = child.walk();
            for specifier in child.children(&mut ds_walker) {
                if specifier.kind() == "delegation_specifier" {
                    if let Some((name, kind)) =
                        parse_single_delegation_specifier(&specifier, content)
                    {
                        parents.push((name, kind));
                    }
                }
            }
        }
    }

    parents
}

/// Parse delegation_specifiers for an interface.
/// Interface parents are always "extends".
fn parse_delegation_specifiers_for_interface(
    decl_node: &tree_sitter::Node,
    content: &str,
) -> Vec<(String, String)> {
    let mut parents = Vec::new();

    let mut walker = decl_node.walk();
    for child in decl_node.children(&mut walker) {
        if child.kind() == "delegation_specifiers" {
            let mut ds_walker = child.walk();
            for specifier in child.children(&mut ds_walker) {
                if specifier.kind() == "delegation_specifier" {
                    if let Some(name) = extract_type_name_from_specifier(&specifier, content) {
                        parents.push((name, "extends".to_string()));
                    }
                }
            }
        }
    }

    parents
}

/// Parse a single delegation_specifier node.
/// Returns (parent_name, "extends"|"implements").
fn parse_single_delegation_specifier(
    specifier: &tree_sitter::Node,
    content: &str,
) -> Option<(String, String)> {
    let mut walker = specifier.walk();
    for child in specifier.children(&mut walker) {
        match child.kind() {
            "constructor_invocation" => {
                // Has parentheses -> extends
                let name = extract_type_name_from_node(&child, content)?;
                return Some((name, "extends".to_string()));
            }
            // "type" is a supertype that resolves to user_type, nullable_type, etc.
            "user_type" => {
                let name = extract_user_type_name(&child, content)?;
                return Some((name, "implements".to_string()));
            }
            "nullable_type" | "parenthesized_type" | "function_type" | "non_nullable_type" => {
                let name = extract_type_name_from_node(&child, content)?;
                return Some((name, "implements".to_string()));
            }
            _ => {}
        }
    }
    None
}

/// Extract the type name from a delegation_specifier (for interface parents)
fn extract_type_name_from_specifier(
    specifier: &tree_sitter::Node,
    content: &str,
) -> Option<String> {
    let mut walker = specifier.walk();
    for child in specifier.children(&mut walker) {
        match child.kind() {
            "constructor_invocation" => {
                return extract_type_name_from_node(&child, content);
            }
            "user_type" => {
                return extract_user_type_name(&child, content);
            }
            "nullable_type" | "parenthesized_type" | "function_type" | "non_nullable_type" => {
                return extract_type_name_from_node(&child, content);
            }
            _ => {}
        }
    }
    None
}

/// Extract the first identifier (type name) from a node by walking its descendants.
/// Used for constructor_invocation and other compound type nodes.
fn extract_type_name_from_node(node: &tree_sitter::Node, content: &str) -> Option<String> {
    let mut found = None;
    walk_tree_preorder(node, |child| {
        if child.kind() == "identifier" {
            found = Some(node_text(content, &child).to_string());
            WalkControl::Stop
        } else {
            WalkControl::Continue
        }
    });
    found
}

/// Extract the name from a user_type node.
/// user_type -> identifier (possibly with type_arguments)
fn extract_user_type_name(node: &tree_sitter::Node, content: &str) -> Option<String> {
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if child.kind() == "identifier" {
            return Some(node_text(content, &child).to_string());
        }
    }
    None
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
    fn suspend_lambda_recovery_preserves_enclosing_and_later_declarations() {
        let content = r#"package t

interface IThing2 : IFeature {
    suspend fun isEnabled(): Boolean

    class Impl : IThing2 {
        override suspend fun isEnabled(): Boolean {
            val default = suspend { true }
            return default()
        }
    }
}

interface LaterFeature

class LaterImpl : LaterFeature
"#;
        let tree = parse_kotlin_tree(content).unwrap();
        assert!(!tree.root_node().has_error());

        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        for (name, kind) in [
            ("IThing2", SymbolKind::Interface),
            ("Impl", SymbolKind::Class),
            ("LaterFeature", SymbolKind::Interface),
            ("LaterImpl", SymbolKind::Class),
        ] {
            assert!(
                symbols
                    .iter()
                    .any(|symbol| symbol.name == name && symbol.kind == kind),
                "missing {kind:?} {name}: {symbols:?}"
            );
        }
        assert_eq!(
            symbols
                .iter()
                .filter(|symbol| symbol.name == "isEnabled")
                .count(),
            2
        );
        assert!(
            !symbols.iter().any(|symbol| symbol.name == "default"),
            "local val leaked into symbols: {symbols:?}"
        );
        let thing = symbols
            .iter()
            .find(|symbol| symbol.name == "IThing2")
            .unwrap();
        assert!(thing
            .parents
            .iter()
            .any(|(parent, kind)| parent == "IFeature" && kind == "extends"));
    }

    #[test]
    fn references_exclude_only_declaration_identifiers() {
        let content = r#"class Engine(val id: String)

fun buildDefaultEngine(): Engine = Engine(id = "default")
fun recurse(): Engine { recurse(); return Engine("same-line") }
"#;
        let (symbols, refs) =
            crate::parsers::parse_file_symbols(content, FileType::Kotlin).unwrap();
        assert!(symbols.iter().any(|symbol| symbol.name == "Engine"));

        let engine_lines: Vec<usize> = refs
            .iter()
            .filter(|reference| reference.name == "Engine")
            .map(|reference| reference.line)
            .collect();
        assert_eq!(engine_lines, vec![3, 3, 4, 4]);
        assert!(refs
            .iter()
            .any(|reference| reference.name == "recurse" && reference.line == 4));
        assert!(!refs
            .iter()
            .any(|reference| reference.name == "Engine" && reference.line == 1));
    }

    #[test]
    fn references_ignore_literals_and_comments_but_keep_interpolation_code() {
        let content = r#"class Widget

fun uses(widget: Widget) {
    Widget() // Widget in an inline comment
    val regular = "Widget ${Widget("Widget")} $widget"
    val raw = """Widget
        ${Widget()}
        Widget"""
    val character = 'W'
    /* Widget in a block comment */
    /** Widget in KDoc */
}
"#;
        let (_, refs) = crate::parsers::parse_file_symbols(content, FileType::Kotlin).unwrap();
        let widget_lines: Vec<usize> = refs
            .iter()
            .filter(|reference| reference.name == "Widget")
            .map(|reference| reference.line)
            .collect();

        assert_eq!(widget_lines, vec![3, 4, 5, 7]);
        assert!(refs
            .iter()
            .filter(|reference| reference.line == 5 || reference.line == 7)
            .all(|reference| reference.context.contains("Widget")));
    }

    #[test]
    fn test_parse_class() {
        let content = "class MyService {\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyService" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_data_class() {
        let content = "data class User(val name: String, val age: Int)\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_object() {
        let content = "object Singleton {\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Singleton" && s.kind == SymbolKind::Object));
    }

    #[test]
    fn test_parse_interface() {
        let content = "interface Repository {\n    fun getAll(): List<Item>\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Repository" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_sealed_interface() {
        let content = "sealed interface Result {\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Result" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_function() {
        let content = "fun processPayment(amount: Double): Boolean {\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "processPayment" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_suspend_function() {
        let content = "    suspend fun fetchData(): Result<Data> {\n    }\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "fetchData" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_property() {
        let content = "    val name: String = \"test\"\n    var count: Int = 0\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "count" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_typealias() {
        let content = "typealias StringMap = Map<String, String>\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "StringMap" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_class_with_inheritance() {
        let content = "class MyFragment(arg: String) : Fragment(), Serializable {\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "MyFragment" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(cls
            .parents
            .iter()
            .any(|(p, k)| p == "Fragment" && k == "extends"));
        assert!(cls
            .parents
            .iter()
            .any(|(p, k)| p == "Serializable" && k == "implements"));
    }

    #[test]
    fn test_parse_class_simple_inheritance() {
        let content = "class Child : Parent {\n}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "Child").unwrap();
        assert!(!cls.parents.is_empty());
    }

    #[test]
    fn test_comments_ignored() {
        let content =
            "// class FakeClass {}\nclass RealClass {}\n/* fun fake() {} */\nfun real() {}\n";
        let symbols = KOTLIN_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(symbols.iter().any(|s| s.name == "real"));
        assert!(!symbols.iter().any(|s| s.name == "fake"));
    }
}
