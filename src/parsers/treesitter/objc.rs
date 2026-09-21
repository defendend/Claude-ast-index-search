//! Tree-sitter based Objective-C parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator};

use super::{line_text, node_text, parse_tree, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static OBJC_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_objc::LANGUAGE.into());

static OBJC_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&OBJC_LANGUAGE, include_str!("queries/objc.scm"))
        .expect("Failed to compile ObjC tree-sitter query")
});

pub static OBJC_PARSER: ObjcParser = ObjcParser;

pub struct ObjcParser;

impl LanguageParser for ObjcParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &OBJC_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*OBJC_QUERY;

        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_class_interface = idx("class_interface");
        let idx_protocol_decl = idx("protocol_decl");
        let idx_class_impl = idx("class_impl");
        let idx_method_decl = idx("method_decl");
        let idx_method_def = idx("method_def");
        let idx_property_decl = idx("property_decl");
        let idx_typedef_decl = idx("typedef_decl");

        // Track class names from @interface to avoid duplicating from @implementation
        let mut interface_names = std::collections::HashSet::new();

        // First pass: collect @interface names
        {
            let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());
            while let Some(m) = matches.next() {
                if let Some(cap) = find_capture(m, idx_class_interface) {
                    if let Some(name) = extract_class_name(content, &cap.node) {
                        // Check if this is a category
                        if cap.node.child_by_field_name("category").is_none() {
                            interface_names.insert(name);
                        }
                    }
                }
            }
        }

        // Reset cursor for second pass
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            // @interface
            if let Some(cap) = find_capture(m, idx_class_interface) {
                let node = &cap.node;
                if let Some(class_name) = extract_class_name(content, node) {
                    let line = node.start_position().row + 1;
                    let sig = line_text(content, line).trim().to_string();

                    // Check if this is a category: @interface ClassName (CategoryName)
                    if node.child_by_field_name("category").is_some() {
                        // Category - use ClassName+Category naming convention
                        symbols.push(ParsedSymbol {
                            name: format!("{}+Category", class_name),
                            kind: SymbolKind::Object,
                            line,
                            signature: sig,
                            parents: vec![(class_name, "extends".to_string())],
                            end_line: None,
                        });
                    } else {
                        let mut parents = Vec::new();

                        // Superclass
                        if let Some(superclass_node) = node.child_by_field_name("superclass") {
                            let superclass = node_text(content, &superclass_node);
                            parents.push((superclass.to_string(), "extends".to_string()));
                        }

                        // Protocol conformance from parameterized_arguments
                        // The <Proto1, Proto2> after superclass is parsed as parameterized_arguments
                        extract_protocols(content, node, &mut parents);

                        symbols.push(ParsedSymbol {
                            name: class_name,
                            kind: SymbolKind::Class,
                            line,
                            signature: sig,
                            parents,
                            end_line: None,
                        });
                    }
                }
                continue;
            }

            // @protocol
            if let Some(cap) = find_capture(m, idx_protocol_decl) {
                let node = &cap.node;
                if let Some(name) = extract_protocol_name(content, node) {
                    let line = node.start_position().row + 1;
                    let sig = line_text(content, line).trim().to_string();
                    let mut parents = Vec::new();

                    // Protocol inheritance from protocol_reference_list
                    extract_protocol_parents(content, node, &mut parents);

                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Interface,
                        line,
                        signature: sig,
                        parents,
                        end_line: None,
                    });
                }
                continue;
            }

            // @implementation
            if let Some(cap) = find_capture(m, idx_class_impl) {
                let node = &cap.node;
                if let Some(name) = extract_class_name(content, node) {
                    let line = node.start_position().row + 1;
                    let sig = line_text(content, line).trim().to_string();

                    // Only add if no @interface already found for this class
                    if !interface_names.contains(&name) {
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::Class,
                            line,
                            signature: sig,
                            parents: vec![],
                            end_line: None,
                        });
                    }
                }
                continue;
            }

            // Method declaration (in @interface/@protocol)
            if let Some(cap) = find_capture(m, idx_method_decl) {
                let node = &cap.node;
                if let Some(name) = extract_method_name(content, node) {
                    let line = node.start_position().row + 1;
                    let sig = line_text(content, line).trim().to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        line,
                        signature: sig,
                        parents: vec![],
                        end_line: None,
                    });
                }
                continue;
            }

            // Method definition (in @implementation)
            if let Some(cap) = find_capture(m, idx_method_def) {
                let node = &cap.node;
                if let Some(name) = extract_method_name(content, node) {
                    let line = node.start_position().row + 1;
                    let sig = line_text(content, line).trim().to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        line,
                        signature: sig,
                        parents: vec![],
                        end_line: None,
                    });
                }
                continue;
            }

            // Property declaration
            if let Some(cap) = find_capture(m, idx_property_decl) {
                let node = &cap.node;
                if let Some(name) = extract_property_name(content, node) {
                    let line = node.start_position().row + 1;
                    let sig = line_text(content, line).trim().to_string();
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Property,
                        line,
                        signature: sig,
                        parents: vec![],
                        end_line: None,
                    });
                }
                continue;
            }

            // Typedef
            if let Some(cap) = find_capture(m, idx_typedef_decl) {
                let node = &cap.node;
                if let Some(name) = extract_typedef_name(content, node) {
                    if !name.is_empty() && name != "NS_ENUM" && name != "NS_OPTIONS" {
                        let line = node.start_position().row + 1;
                        let sig = line_text(content, line).trim().to_string();
                        symbols.push(ParsedSymbol {
                            name,
                            kind: SymbolKind::TypeAlias,
                            line,
                            signature: sig,
                            parents: vec![],
                            end_line: None,
                        });
                    }
                }
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Extract the class/implementation name (first identifier child after @interface/@implementation keyword)
fn extract_class_name(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(content, &child).to_string());
        }
    }
    None
}

/// Extract protocol name from protocol_declaration
/// The protocol name is the first identifier child after @protocol keyword
fn extract_protocol_name(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "type_identifier" {
            return Some(node_text(content, &child).to_string());
        }
    }
    None
}

/// Extract protocol conformance from parameterized_arguments in class_interface.
/// The <Proto1, Proto2> list appears as:
///   parameterized_arguments > type_name > type_identifier
fn extract_protocols(content: &str, node: &Node, parents: &mut Vec<(String, String)>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "parameterized_arguments" {
            extract_type_names_as_protocols(content, &child, parents);
        }
    }
}

/// Recursively extract type identifiers from parameterized_arguments children
fn extract_type_names_as_protocols(
    content: &str,
    node: &Node,
    parents: &mut Vec<(String, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_name" => {
                // type_name > type_identifier
                let mut tn_cursor = child.walk();
                for tn_child in child.children(&mut tn_cursor) {
                    match tn_child.kind() {
                        "type_identifier"
                        | "identifier"
                        | "typedefed_identifier"
                        | "type_specifier" => {
                            let proto = node_text(content, &tn_child).to_string();
                            if !proto.is_empty() {
                                parents.push((proto, "implements".to_string()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "type_identifier" | "identifier" => {
                let proto = node_text(content, &child).to_string();
                if !proto.is_empty() {
                    parents.push((proto, "implements".to_string()));
                }
            }
            _ => {}
        }
    }
}

/// Extract parent protocols from protocol_reference_list in protocol_declaration
fn extract_protocol_parents(content: &str, node: &Node, parents: &mut Vec<(String, String)>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "protocol_reference_list" {
            let mut inner_cursor = child.walk();
            for inner_child in child.children(&mut inner_cursor) {
                if inner_child.kind() == "identifier" || inner_child.kind() == "type_identifier" {
                    let name = node_text(content, &inner_child).to_string();
                    if !name.is_empty() {
                        parents.push((name, "extends".to_string()));
                    }
                }
            }
        }
    }
}

/// Extract method name from method_declaration or method_definition
/// For simple methods like `- (void)viewDidLoad`, the name is the first identifier child.
/// For methods with parameters like `- (void)setName:(NSString *)name`, we extract
/// the first selector part from keyword_declarator or method_selector.
fn extract_method_name(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                return Some(node_text(content, &child).to_string());
            }
            // method_selector_no_list or keyword_selector contains the selector parts
            "method_selector_no_list" | "keyword_selector" | "method_selector" => {
                return extract_selector_name(content, &child);
            }
            "keyword_declarator" => {
                // First keyword_declarator contains the primary method name
                return extract_first_keyword_name(content, &child);
            }
            _ => {}
        }
    }
    None
}

/// Extract selector name from a method_selector_no_list or keyword_selector node
fn extract_selector_name(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                return Some(node_text(content, &child).to_string());
            }
            "keyword_selector" => {
                return extract_selector_name(content, &child);
            }
            "keyword_declarator" => {
                return extract_first_keyword_name(content, &child);
            }
            _ => {}
        }
    }
    None
}

/// Extract the method name from the first keyword_declarator
/// keyword_declarator contains identifiers - the first one is typically the selector name part
fn extract_first_keyword_name(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(content, &child).to_string());
        }
    }
    None
}

/// Extract property name from property_declaration
/// The property name is in the struct_declaration > struct_declarator chain
/// or can be found by looking for the last identifier in the property declaration
fn extract_property_name(content: &str, node: &Node) -> Option<String> {
    // Try to find struct_declaration first
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "struct_declaration" {
            return extract_name_from_struct_declaration(content, &child);
        }
        if child.kind() == "atomic_declaration" {
            return extract_last_identifier(content, &child);
        }
    }
    // Fallback: try to extract from the line text using simple parsing
    extract_property_name_from_text(content, node)
}

/// Extract name from struct_declaration (used in property)
fn extract_name_from_struct_declaration(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "struct_declarator" {
            return extract_declarator_name(content, &child);
        }
    }
    None
}

/// Extract identifier from a declarator node, walking through pointer_declarator etc.
fn extract_declarator_name(content: &str, node: &Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "_field_identifier" | "field_identifier" => {
                return Some(node_text(content, &child).to_string());
            }
            "pointer_declarator"
            | "parenthesized_declarator"
            | "array_declarator"
            | "function_declarator"
            | "block_pointer_declarator" => {
                return extract_declarator_name(content, &child);
            }
            _ => {}
        }
    }
    // If no identifier found in children, check if the node itself is an identifier
    if node.kind() == "identifier" || node.kind() == "field_identifier" {
        return Some(node_text(content, node).to_string());
    }
    None
}

/// Extract last identifier from a node (fallback)
fn extract_last_identifier(content: &str, node: &Node) -> Option<String> {
    let mut last_id = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "field_identifier" {
            last_id = Some(node_text(content, &child).to_string());
        }
    }
    last_id
}

/// Fallback: extract property name from the line text
fn extract_property_name_from_text(content: &str, node: &Node) -> Option<String> {
    let line = node.start_position().row + 1;
    let text = line_text(content, line);
    // Pattern: @property (...) Type *name;  or  @property (...) Type name;
    // Find the last word before the semicolon
    let text = text.trim().trim_end_matches(';').trim();
    // Remove pointer asterisks
    let text = text.trim_end_matches('*').trim();
    // The last word is the property name
    text.rsplit_once(|c: char| c.is_whitespace() || c == '*')
        .map(|(_, name)| name.trim_matches('*').to_string())
        .filter(|n| !n.is_empty())
}

/// Extract typedef name from type_definition node
/// The name is in the `declarator` field
fn extract_typedef_name(content: &str, node: &Node) -> Option<String> {
    // Try the declarator field first
    if let Some(decl) = node.child_by_field_name("declarator") {
        if decl.kind() == "identifier" || decl.kind() == "type_identifier" {
            return Some(node_text(content, &decl).to_string());
        }
        return extract_declarator_name(content, &decl);
    }

    // Fallback: walk children to find a _type_declarator or identifier
    let mut last_identifier = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "type_identifier" => {
                last_identifier = Some(node_text(content, &child).to_string());
            }
            "type_declarator" | "_type_declarator" | "pointer_declarator" => {
                if let Some(name) = extract_declarator_name(content, &child) {
                    last_identifier = Some(name);
                }
            }
            _ => {}
        }
    }

    // If still not found, try parsing from text
    if last_identifier.is_none() {
        last_identifier = extract_typedef_name_from_text(content, node);
    }

    last_identifier
}

/// Fallback: extract typedef name from line text
fn extract_typedef_name_from_text(content: &str, node: &Node) -> Option<String> {
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    // Get the text of the entire typedef (could be multiline)
    let lines: Vec<&str> = content
        .lines()
        .skip(start_line - 1)
        .take(end_line - start_line + 1)
        .collect();
    let full_text = lines.join(" ");
    // Find name before the last semicolon: typedef ... Name;
    let text = full_text.trim().trim_end_matches(';').trim();
    text.rsplit_once(|c: char| c.is_whitespace() || c == '}' || c == '*')
        .map(|(_, name)| name.trim().to_string())
        .filter(|n| !n.is_empty() && n.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false))
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
    fn test_parse_interface_with_superclass_and_protocols() {
        let content =
            "@interface MyView : UIView <UITableViewDelegate, UITableViewDataSource>\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "MyView" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "UIView" && k == "extends"),
            "expected superclass UIView, got parents: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "UITableViewDelegate" && k == "implements"),
            "expected protocol UITableViewDelegate, got parents: {:?}",
            cls.parents
        );
        assert!(
            cls.parents
                .iter()
                .any(|(p, k)| p == "UITableViewDataSource" && k == "implements"),
            "expected protocol UITableViewDataSource, got parents: {:?}",
            cls.parents
        );
    }

    #[test]
    fn test_parse_interface_no_superclass() {
        let content = "@interface MyRoot\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "MyRoot" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(
            cls.parents.is_empty(),
            "expected no parents, got: {:?}",
            cls.parents
        );
    }

    #[test]
    fn test_parse_interface_only_superclass() {
        let content = "@interface MyChild : NSObject\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "MyChild" && s.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(cls.parents.len(), 1);
        assert!(cls
            .parents
            .iter()
            .any(|(p, k)| p == "NSObject" && k == "extends"));
    }

    #[test]
    fn test_parse_category() {
        let content = "@interface NSString (Utilities)\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let cat = symbols
            .iter()
            .find(|s| s.name == "NSString+Category")
            .unwrap();
        assert_eq!(cat.kind, SymbolKind::Object);
        assert!(cat.parents.iter().any(|(p, _)| p == "NSString"));
    }

    #[test]
    fn test_parse_protocol() {
        let content = "@protocol Fetchable <NSObject>\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let p = symbols.iter().find(|s| s.name == "Fetchable").unwrap();
        assert_eq!(p.kind, SymbolKind::Interface);
        assert!(
            p.parents
                .iter()
                .any(|(p, k)| p == "NSObject" && k == "extends"),
            "expected parent NSObject, got: {:?}",
            p.parents
        );
    }

    #[test]
    fn test_parse_protocol_no_parent() {
        let content = "@protocol Drawable\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let p = symbols.iter().find(|s| s.name == "Drawable").unwrap();
        assert_eq!(p.kind, SymbolKind::Interface);
        assert!(p.parents.is_empty());
    }

    #[test]
    fn test_parse_protocol_multiple_parents() {
        let content = "@protocol Combined <NSObject, NSCoding, NSCopying>\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let p = symbols.iter().find(|s| s.name == "Combined").unwrap();
        assert_eq!(p.kind, SymbolKind::Interface);
        assert!(p.parents.iter().any(|(n, _)| n == "NSObject"));
        assert!(p.parents.iter().any(|(n, _)| n == "NSCoding"));
        assert!(p.parents.iter().any(|(n, _)| n == "NSCopying"));
    }

    #[test]
    fn test_parse_implementation() {
        let content = "@implementation MyService\n- (void)doWork {\n}\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let cls = symbols
            .iter()
            .find(|s| s.name == "MyService" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(cls.parents.is_empty());
    }

    #[test]
    fn test_implementation_skipped_if_interface_exists() {
        let content = "@interface MyClass : NSObject\n@end\n@implementation MyClass\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        let count = symbols
            .iter()
            .filter(|s| s.name == "MyClass" && s.kind == SymbolKind::Class)
            .count();
        assert_eq!(count, 1, "should not duplicate class from @implementation");
    }

    #[test]
    fn test_parse_method_declaration() {
        let content = "@interface MyClass : NSObject\n- (void)viewDidLoad;\n+ (instancetype)sharedInstance;\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "viewDidLoad" && s.kind == SymbolKind::Function),
            "expected viewDidLoad, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "sharedInstance" && s.kind == SymbolKind::Function),
            "expected sharedInstance, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_method_definition() {
        let content = "@implementation MyService\n- (void)doWork {\n    NSLog(@\"working\");\n}\n+ (instancetype)shared {\n    return nil;\n}\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "doWork" && s.kind == SymbolKind::Function),
            "expected doWork, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "shared" && s.kind == SymbolKind::Function),
            "expected shared, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_property() {
        let content =
            "@interface MyClass : NSObject\n@property (nonatomic, strong) NSString *name;\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "name" && s.kind == SymbolKind::Property),
            "expected property 'name', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_property_no_attributes() {
        let content = "@interface Config\n@property NSInteger count;\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "count" && s.kind == SymbolKind::Property),
            "expected property 'count', got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_parse_typedef_struct() {
        let content = "typedef struct { int x; int y; } CGPoint;\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "CGPoint" && s.kind == SymbolKind::TypeAlias),
            "expected typedef CGPoint, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_comments_ignored() {
        let content = "// @interface FakeClass : NSObject\n@interface RealClass : NSObject\n@end\n";
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(
            !symbols.iter().any(|s| s.name == "FakeClass"),
            "should not parse class from comment, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_full_objc_file() {
        let content = r#"
@interface MyView : UIView <UITableViewDelegate, UITableViewDataSource>
@property (nonatomic, strong) NSString *title;
- (void)viewDidLoad;
+ (instancetype)sharedInstance;
@end

@interface NSString (Utilities)
@end

@protocol Fetchable <NSObject>
- (void)fetchData;
@end

@implementation MyService
- (void)doWork {
}
@end

typedef struct { int x; int y; } CGPoint;
"#;
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();

        // Class with superclass and protocols
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyView" && s.kind == SymbolKind::Class));

        // Category
        assert!(symbols
            .iter()
            .any(|s| s.name == "NSString+Category" && s.kind == SymbolKind::Object));

        // Protocol
        assert!(symbols
            .iter()
            .any(|s| s.name == "Fetchable" && s.kind == SymbolKind::Interface));

        // Methods
        assert!(symbols
            .iter()
            .any(|s| s.name == "viewDidLoad" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "sharedInstance" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "fetchData" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "doWork" && s.kind == SymbolKind::Function));

        // Property
        assert!(symbols
            .iter()
            .any(|s| s.name == "title" && s.kind == SymbolKind::Property));

        // Implementation (MyService has no @interface, so it should appear as Class)
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyService" && s.kind == SymbolKind::Class));

        // Typedef
        assert!(symbols
            .iter()
            .any(|s| s.name == "CGPoint" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_multiple_properties() {
        let content = r#"
@interface Person : NSObject
@property (nonatomic, strong) NSString *firstName;
@property (nonatomic, strong) NSString *lastName;
@property (nonatomic, assign) NSInteger age;
@end
"#;
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "firstName" && s.kind == SymbolKind::Property),
            "expected firstName, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "lastName" && s.kind == SymbolKind::Property),
            "expected lastName, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "age" && s.kind == SymbolKind::Property),
            "expected age, got: {:?}",
            symbols
        );
    }

    #[test]
    fn test_protocol_methods() {
        let content = r#"
@protocol DataSource <NSObject>
- (NSInteger)numberOfItems;
+ (NSString *)defaultTitle;
@end
"#;
        let symbols = OBJC_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "DataSource" && s.kind == SymbolKind::Interface));
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "numberOfItems" && s.kind == SymbolKind::Function),
            "expected numberOfItems, got: {:?}",
            symbols
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "defaultTitle" && s.kind == SymbolKind::Function),
            "expected defaultTitle, got: {:?}",
            symbols
        );
    }
}
