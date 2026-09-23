//! Tree-sitter based Swift parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{
    line_text, node_line, node_text, parse_tree, walk_tree_preorder, LanguageParser, WalkControl,
};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static SWIFT_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_swift::LANGUAGE.into());

static SWIFT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&SWIFT_LANGUAGE, include_str!("queries/swift.scm"))
        .expect("Failed to compile Swift tree-sitter query")
});

pub static SWIFT_PARSER: SwiftParser = SwiftParser;

pub struct SwiftParser;

impl LanguageParser for SwiftParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &SWIFT_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*SWIFT_QUERY;

        // Build capture name -> index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_decl_kind = idx("decl_kind");
        let idx_class_name = idx("class_name");
        let idx_enum_name = idx("enum_name");
        let idx_ext_type = idx("ext_type");
        let idx_protocol_name = idx("protocol_name");
        let idx_func_name = idx("func_name");
        let idx_init_name = idx("init_name");
        let idx_prop_name = idx("prop_name");
        let idx_typealias_name = idx("typealias_name");
        let idx_import_name = idx("import_name");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // Import: the imported module name (a Swift module is its target name)
            if let Some(cap) = find_capture(m, idx_import_name) {
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: node_text(content, &cap.node).to_string(),
                    kind: SymbolKind::Import,
                    line,
                    end_line: None,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                });
                continue;
            }

            // Class / Struct / Actor
            if let Some(name_cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);

                // Determine kind from declaration_kind
                let decl_kind_str = find_capture(m, idx_decl_kind)
                    .map(|dk_cap| node_text(content, &dk_cap.node))
                    .unwrap_or("class");
                let kind = SymbolKind::Class;

                // Structs and actors can't have superclasses — all parents are protocol conformances
                let all_implements = matches!(decl_kind_str, "struct" | "actor");

                // Walk the class_declaration node for inheritance_specifier children
                let parents = if let Some(decl_node) = name_cap.node.parent() {
                    collect_parents_from_node(&decl_node, content, all_implements)
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line: None,
                });
                continue;
            }

            // Enum
            if let Some(name_cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                // Enums can't have superclasses — all parents are protocol conformances / raw values
                let parents = if let Some(decl_node) = name_cap.node.parent() {
                    collect_parents_from_node(&decl_node, content, true)
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line: None,
                });
                continue;
            }

            // Extension
            if let Some(ext_cap) = find_capture(m, idx_ext_type) {
                let type_name = node_text(content, &ext_cap.node);
                // Strip generic parameters if present
                let base_name = type_name.split('<').next().unwrap_or(type_name).trim();
                // `extension Outer.Inner` extends Inner; keep the simple name so
                // hierarchy/implementations lookups by type name find it.
                let base_name = base_name.rsplit('.').next().unwrap_or(base_name).trim();
                let extended_name = format!("{}+Extension", base_name);
                let line = node_line(&ext_cap.node);

                // Collect conformances from extension declaration
                let mut parents = vec![(base_name.to_string(), "extends".to_string())];
                if let Some(decl_node) = ext_cap.node.parent() {
                    let conformances = collect_parents_from_node(&decl_node, content, true);
                    parents.extend(conformances);
                }

                symbols.push(ParsedSymbol {
                    name: extended_name,
                    kind: SymbolKind::Object,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line: None,
                });
                continue;
            }

            // Protocol
            if let Some(name_cap) = find_capture(m, idx_protocol_name) {
                let name = node_text(content, &name_cap.node);
                let line = node_line(&name_cap.node);
                // Protocol parents are all protocol conformances
                let parents = if let Some(decl_node) = name_cap.node.parent() {
                    collect_parents_from_node(&decl_node, content, true)
                } else {
                    vec![]
                };

                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents,
                    end_line: None,
                });
                continue;
            }

            // Function
            if let Some(cap) = find_capture(m, idx_func_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                // Extract multi-line signature from the function_declaration node
                let signature = if let Some(func_node) = cap.node.parent() {
                    extract_func_signature(content, &func_node)
                } else {
                    line_text(content, line).trim().to_string()
                };
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature,
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // Init
            if let Some(cap) = find_capture(m, idx_init_name) {
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: "init".to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }

            // Property (members and globals only, like Kotlin: locals are not symbols)
            if let Some(cap) = find_capture(m, idx_prop_name) {
                if is_local_declaration(&cap.node) {
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
                    end_line: None,
                });
                continue;
            }

            // TypeAlias
            if let Some(cap) = find_capture(m, idx_typealias_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::TypeAlias,
                    line,
                    signature: line_text(content, line).trim().to_string(),
                    parents: vec![],
                    end_line: None,
                });
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Collect parent types by walking a declaration node's inheritance_specifier children.
/// If `all_implements` is true (structs, enums, actors, protocols), all parents are "implements".
/// Otherwise (classes), the first parent is "extends" and the rest are "implements".
fn collect_parents_from_node(
    node: &tree_sitter::Node,
    content: &str,
    all_implements: bool,
) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "inheritance_specifier" {
            // Find the type_identifier inside user_type
            if let Some(type_name) = find_type_identifier_in(&child, content) {
                let kind = if all_implements || !parents.is_empty() {
                    "implements"
                } else {
                    "extends"
                };
                parents.push((type_name, kind.to_string()));
            }
        }
    }
    parents
}

/// Extract a function signature from the function_declaration node,
/// spanning multiple lines up to (but not including) the body `{`.
fn extract_func_signature(content: &str, func_node: &tree_sitter::Node) -> String {
    let start = func_node.start_position();
    let end = func_node.end_position();
    let lines: Vec<&str> = content.lines().collect();

    let mut sig_parts = Vec::new();
    for line in &lines[start.row..=end.row.min(lines.len().saturating_sub(1))] {
        let line = line.trim();
        // Stop at the body opening brace
        if let Some(brace_pos) = line.find('{') {
            let before = line[..brace_pos].trim();
            if !before.is_empty() {
                sig_parts.push(before);
            }
            break;
        }
        if !line.is_empty() {
            sig_parts.push(line);
        }
    }

    let sig = sig_parts.join(" ");
    if sig.is_empty() {
        lines
            .get(start.row)
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    } else {
        sig
    }
}

/// Find the first type_identifier in a node's descendants
fn find_type_identifier_in(node: &tree_sitter::Node, content: &str) -> Option<String> {
    let mut found = None;
    walk_tree_preorder(node, |child| {
        // A module-qualified type (`Module.Proto`) is a user_type with several
        // type_identifier children; the last one is the type itself.
        if child.kind() == "user_type" {
            let mut cursor = child.walk();
            let last = child
                .children(&mut cursor)
                .filter(|c| c.kind() == "type_identifier")
                .last();
            if let Some(last) = last {
                found = Some(node_text(content, &last).trim().to_string());
                return WalkControl::Stop;
            }
        }
        if child.kind() == "type_identifier" {
            let name = node_text(content, &child);
            let name = name.split('<').next().unwrap_or(name).trim();
            if !name.is_empty() {
                found = Some(name.to_string());
                return WalkControl::Stop;
            }
        }
        WalkControl::Continue
    });
    found
}

/// Whether a declaration sits inside executable code (function or initializer
/// body, closure, accessor) rather than a type body or at file scope.
fn is_local_declaration(node: &tree_sitter::Node) -> bool {
    let mut current = node.parent();
    while let Some(n) = current {
        match n.kind() {
            "function_body" | "lambda_literal" | "computed_property" | "computed_getter"
            | "computed_setter" | "computed_modify" | "willset_didset_block" | "statements" => {
                return true
            }
            "class_body" | "protocol_body" | "enum_class_body" | "source_file" => return false,
            _ => current = n.parent(),
        }
    }
    false
}

/// Find a capture by index in a match
fn find_capture<'a>(
    m: &'a tree_sitter::QueryMatch<'a, 'a>,
    idx: Option<u32>,
) -> Option<&'a tree_sitter::QueryCapture<'a>> {
    let idx = idx?;
    m.captures.iter().find(|c| c.index == idx)
}

/// A SwiftUI property wrapper found by tree-sitter.
#[derive(Debug)]
pub struct SwiftPropertyWrapper {
    /// The wrapper name, e.g. "State", "Environment", "AppStorage"
    pub wrapper: String,
    /// The property name
    pub name: String,
    /// 1-based line number
    pub line: usize,
    /// Full line text (trimmed)
    pub text: String,
}

/// Find all properties with `@`-attribute wrappers in Swift source using tree-sitter.
/// This replaces the regex-based approach and automatically handles any property wrapper
/// (including `@Environment(\.dismiss)`, `@AppStorage("key")`, `@Bindable`, etc.).
pub fn find_property_wrappers(content: &str) -> Result<Vec<SwiftPropertyWrapper>> {
    let tree = parse_tree(content, &SWIFT_LANGUAGE)?;
    let mut results = Vec::new();

    // Walk all property_declaration nodes
    walk_for_kind(&tree.root_node(), "property_declaration", &mut |node| {
        // Look for modifiers > attribute > user_type > type_identifier
        let mut wrapper_name = None;
        let mut prop_name = None;
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                let mut mc = child.walk();
                for mod_child in child.children(&mut mc) {
                    if mod_child.kind() == "attribute" {
                        // Find the type_identifier inside the attribute
                        if let Some(name) = find_type_identifier_in(&mod_child, content) {
                            wrapper_name = Some(name);
                        }
                    }
                }
            }
            if child.kind() == "pattern" {
                if let Some(id) = child.child(0) {
                    if id.kind() == "simple_identifier" {
                        prop_name = Some(node_text(content, &id).to_string());
                    }
                }
            }
        }

        if let (Some(wrapper), Some(name)) = (wrapper_name, prop_name) {
            let line = node_line(node);
            results.push(SwiftPropertyWrapper {
                wrapper,
                name,
                line,
                text: line_text(content, line).trim().to_string(),
            });
        }
    });

    Ok(results)
}

/// An async Swift function found by tree-sitter.
#[derive(Debug)]
pub struct SwiftAsyncFunc {
    /// Function name
    pub name: String,
    /// 1-based line number
    pub line: usize,
    /// Full signature text
    pub signature: String,
}

/// Find all async functions in Swift source using tree-sitter.
/// Handles multi-line signatures natively since tree-sitter parses the full AST.
pub fn find_async_funcs(content: &str) -> Result<Vec<SwiftAsyncFunc>> {
    let tree = parse_tree(content, &SWIFT_LANGUAGE)?;
    let mut results = Vec::new();

    walk_for_kind(&tree.root_node(), "function_declaration", &mut |node| {
        // Check if the function has an `async` child node
        let mut has_async = false;
        let mut func_name = None;
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.kind() == "async" {
                has_async = true;
            }
            if child.kind() == "simple_identifier" {
                func_name = Some(node_text(content, &child).to_string());
            }
        }

        if has_async {
            if let Some(name) = func_name {
                let line = node_line(node);
                results.push(SwiftAsyncFunc {
                    name,
                    line,
                    signature: extract_func_signature(content, node),
                });
            }
        }
    });

    Ok(results)
}

/// Walk tree recursively, calling `callback` for every node matching `kind`.
fn walk_for_kind<'a>(
    node: &tree_sitter::Node<'a>,
    kind: &str,
    callback: &mut dyn FnMut(&tree_sitter::Node<'a>),
) {
    walk_tree_preorder(node, |current| {
        if current.kind() == kind {
            callback(&current);
        }
        WalkControl::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = "class ViewController: UIViewController {\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "ViewController").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.iter().any(|(p, _)| p == "UIViewController"));
    }

    #[test]
    fn test_parse_public_final_class() {
        let content = "public final class AppDelegate: UIResponder, UIApplicationDelegate {\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "AppDelegate").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls
            .parents
            .iter()
            .any(|(p, k)| p == "UIResponder" && k == "extends"));
        assert!(cls
            .parents
            .iter()
            .any(|(p, k)| p == "UIApplicationDelegate" && k == "implements"));
    }

    #[test]
    fn test_parse_struct() {
        let content = "struct User: Codable, Equatable {\n    let id: Int\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let s = symbols.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(s.kind, SymbolKind::Class); // struct treated as class
        assert!(s.parents.iter().any(|(p, _)| p == "Codable"));
    }

    #[test]
    fn test_parse_enum() {
        let content = "enum Direction: String, CaseIterable {\n    case north, south\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let e = symbols.iter().find(|s| s.name == "Direction").unwrap();
        assert_eq!(e.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_parse_protocol() {
        let content = "protocol Fetchable: AnyObject {\n    func fetch() async\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let p = symbols.iter().find(|s| s.name == "Fetchable").unwrap();
        assert_eq!(p.kind, SymbolKind::Interface);
        assert!(p.parents.iter().any(|(p, _)| p == "AnyObject"));
    }

    #[test]
    fn test_parse_actor() {
        let content = "actor DataStore {\n    func save() {}\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let a = symbols.iter().find(|s| s.name == "DataStore").unwrap();
        assert_eq!(a.kind, SymbolKind::Class); // actor treated as class
    }

    #[test]
    fn test_parse_extension() {
        let content =
            "extension String: CustomProtocol {\n    func trimmed() -> String { self }\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let ext = symbols
            .iter()
            .find(|s| s.name == "String+Extension")
            .unwrap();
        assert_eq!(ext.kind, SymbolKind::Object);
        assert!(ext
            .parents
            .iter()
            .any(|(p, k)| p == "String" && k == "extends"));
    }

    #[test]
    fn test_parse_function() {
        let content = "func loadData(id: Int) async throws -> Data {\n    fatalError()\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "loadData").unwrap();
        assert_eq!(f.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_init() {
        let content =
            "class Foo {\n    public init(name: String) {\n        self.name = name\n    }\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "init" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_property() {
        let content = "class Foo {\n    var name: String = \"\"\n    let count: Int = 0\n    static var shared: Foo = Foo()\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "count" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "shared" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_typealias() {
        let content = "public typealias Completion = (Result<Data, Error>) -> Void\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let ta = symbols.iter().find(|s| s.name == "Completion").unwrap();
        assert_eq!(ta.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_nested_function() {
        let content = "class ViewController {\n    func loadData() async throws -> Data {\n        fatalError()\n    }\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "loadData" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_generic_class() {
        let content = "class Container<T>: Sequence {\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "Container").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.iter().any(|(p, _)| p == "Sequence"));
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// class FakeClass {}\nclass RealClass {\n}\n/* func fakeFunc() {} */\nfunc realFunc() {}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(symbols.iter().any(|s| s.name == "realFunc"));
        assert!(!symbols.iter().any(|s| s.name == "fakeFunc"));
    }

    #[test]
    fn test_local_variables_are_not_symbols() {
        let content = "let global = 0\nclass A {\n  let field = 1\n  var computed: Int { let tmp = 2; return tmp }\n  func f() {\n    let local = 3\n    run { var inClosure = 4 }\n  }\n  init() { let inInit = 5 }\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let properties: Vec<&str> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Property)
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(properties, ["global", "field", "computed"]);
    }

    #[test]
    fn test_imports_record_module_names() {
        let content = "import UIKit\n@testable import Foo.Bar\nimport struct Baz.Qux\n#if canImport(X)\nimport X\n#endif\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let imports: Vec<(&str, usize)> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Import)
            .map(|s| (s.name.as_str(), s.line))
            .collect();
        assert_eq!(imports, [("UIKit", 1), ("Foo", 2), ("Baz", 3), ("X", 5)]);
    }

    #[test]
    fn test_module_qualified_conformances_resolve_to_simple_names() {
        let content = "class A: UIKit.UIView, Sdk.Listener<Int> {}\nextension Outer.Inner: Sdk.Proto {}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let a = symbols.iter().find(|s| s.name == "A").unwrap();
        assert_eq!(
            a.parents,
            [
                ("UIView".to_string(), "extends".to_string()),
                ("Listener".to_string(), "implements".to_string()),
            ]
        );
        let ext = symbols.iter().find(|s| s.name == "Inner+Extension").unwrap();
        assert!(ext.parents.contains(&("Proto".to_string(), "implements".to_string())));
    }

    #[test]
    fn test_parse_class_no_parents() {
        let content = "class Empty {\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let cls = symbols.iter().find(|s| s.name == "Empty").unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.parents.is_empty());
    }

    // === Issue #1: Incorrect parent classification for structs/enums ===

    #[test]
    fn test_struct_parents_all_implements() {
        // Structs can't have superclasses — all parents are protocol conformances
        let content = "struct User: Codable, Equatable {\n    let id: Int\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let s = symbols.iter().find(|s| s.name == "User").unwrap();
        // Both should be "implements", not first="extends"
        assert!(
            s.parents.iter().all(|(_, k)| k == "implements"),
            "struct parents should all be 'implements', got: {:?}",
            s.parents
        );
    }

    #[test]
    fn test_enum_raw_value_not_extends() {
        // enum Foo: String — String is RawRepresentable conformance, not superclass
        let content = "enum Direction: String, CaseIterable {\n    case north\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let e = symbols.iter().find(|s| s.name == "Direction").unwrap();
        assert!(
            e.parents.iter().all(|(_, k)| k == "implements"),
            "enum parents should all be 'implements', got: {:?}",
            e.parents
        );
    }

    #[test]
    fn test_actor_parents_all_implements() {
        // Actors can't inherit from classes — all parents are protocol conformances
        let content = "actor DataStore: Sendable, CustomStringConvertible {\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let a = symbols.iter().find(|s| s.name == "DataStore").unwrap();
        assert!(
            a.parents.iter().all(|(_, k)| k == "implements"),
            "actor parents should all be 'implements', got: {:?}",
            a.parents
        );
    }

    // === Issue #4: Signature is single-line ===

    #[test]
    fn test_multiline_func_signature() {
        let content = r#"
public func configure(
    with model: ViewModel,
    animated: Bool
) -> Result<Void, Error> {
    fatalError()
}
"#;
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "configure").unwrap();
        // Signature should contain the full declaration, not just the first line
        assert!(
            f.signature.contains("animated: Bool"),
            "signature should include all parameters, got: {:?}",
            f.signature
        );
    }

    #[test]
    fn test_protocol_func_signature_no_body() {
        let content = r#"
protocol Service {
    func fetchItems(
        matching query: String,
        limit: Int
    ) async throws -> [Item]
}
"#;
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "fetchItems").unwrap();
        // Protocol functions have no body — signature should still capture all parameters
        assert!(
            f.signature.contains("limit: Int"),
            "protocol func signature should include all parameters, got: {:?}",
            f.signature
        );
        assert!(
            f.signature.contains("async throws"),
            "protocol func signature should include async throws, got: {:?}",
            f.signature
        );
    }

    // === Issue #8: Extension conformances not captured ===

    #[test]
    fn test_extension_conformances_captured() {
        let content = "extension MyStruct: Codable, Equatable {\n}\n";
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();
        let ext = symbols
            .iter()
            .find(|s| s.name == "MyStruct+Extension")
            .unwrap();
        // Extension should record all protocol conformances, not just the base type
        assert!(
            ext.parents.iter().any(|(p, _)| p == "Codable"),
            "extension should capture Codable conformance, got: {:?}",
            ext.parents
        );
        assert!(
            ext.parents.iter().any(|(p, _)| p == "Equatable"),
            "extension should capture Equatable conformance, got: {:?}",
            ext.parents
        );
    }

    #[test]
    fn test_parse_multiple_declarations() {
        let content = r#"
class ViewController: UIViewController, UITableViewDelegate {
    var name: String = ""
    let count: Int = 0
    func loadData() async throws -> Data { fatalError() }
    init(name: String) { self.name = name }
}
struct User: Codable { let id: Int }
enum Direction: String { case north }
protocol Fetchable: AnyObject { func fetch() async }
actor DataStore { func save() {} }
extension String { func trimmed() -> String { self } }
typealias Completion = (Result<Data, Error>) -> Void
"#;
        let symbols = SWIFT_PARSER.parse_symbols(content).unwrap();

        // Check that all major declarations are found
        assert!(symbols
            .iter()
            .any(|s| s.name == "ViewController" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Direction" && s.kind == SymbolKind::Enum));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Fetchable" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "DataStore" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "String+Extension" && s.kind == SymbolKind::Object));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Completion" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols
            .iter()
            .any(|s| s.name == "loadData" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "init" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "count" && s.kind == SymbolKind::Property));
    }
}
