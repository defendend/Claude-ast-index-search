//! Tree-sitter based C# parser

use anyhow::Result;
use std::sync::LazyLock;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator};

use super::{node_line, node_text, parse_tree, signature_line, text_end_line, LanguageParser};
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;

static CSHARP_LANGUAGE: LazyLock<Language> = LazyLock::new(|| tree_sitter_c_sharp::LANGUAGE.into());

static CSHARP_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&CSHARP_LANGUAGE, include_str!("queries/csharp.scm"))
        .expect("Failed to compile C# tree-sitter query")
});

pub static CSHARP_PARSER: CSharpParser = CSharpParser;

pub struct CSharpParser;

/// Significant C# attributes that are worth tracking
fn is_significant_attr(name: &str) -> bool {
    matches!(
        name,
        "Serializable"
            | "DataContract"
            | "DataMember"
            | "JsonProperty"
            | "JsonIgnore"
            | "Required"
            | "Authorize"
            | "AllowAnonymous"
            | "HttpGet"
            | "HttpPost"
            | "HttpPut"
            | "HttpDelete"
            | "Route"
            | "ApiController"
            | "Controller"
            | "Test"
            | "TestMethod"
            | "Fact"
            | "Theory"
            | "SerializeField"
            | "Header"
            | "Tooltip"
            | "Range"
            | "DllImport"
            | "StructLayout"
            | "MarshalAs"
            | "Obsolete"
            | "Conditional"
            | "DebuggerDisplay"
    )
}

/// Check if a C# name looks like an interface (starts with I + uppercase)
fn is_interface_name(name: &str) -> bool {
    name.starts_with('I')
        && name.len() > 1
        && name
            .chars()
            .nth(1)
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
}

/// Parse base_list node to extract parent type names with their relationship kind.
/// In C#, the base_list contains types separated by commas.
/// Convention: names starting with I+uppercase are "implements", others are "extends".
fn parse_base_list(content: &str, node: &tree_sitter::Node) -> Vec<(String, String)> {
    let mut parents = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let kind = child.kind();

        // Skip punctuation and argument_list
        if kind == "," || kind == ":" || kind == "argument_list" {
            continue;
        }

        // For primary_constructor_base_type (e.g. `Person(Name)` in record bases),
        // extract the type from the first named child
        if kind == "primary_constructor_base_type" {
            let mut inner_cursor = child.walk();
            for inner_child in child.children(&mut inner_cursor) {
                let inner_kind = inner_child.kind();
                if inner_kind != "argument_list"
                    && inner_kind != ","
                    && inner_kind != "("
                    && inner_kind != ")"
                {
                    let type_name = extract_type_name(content, &inner_child);
                    if !type_name.is_empty() {
                        let rel = if is_interface_name(&type_name) {
                            "implements".to_string()
                        } else {
                            "extends".to_string()
                        };
                        parents.push((type_name, rel));
                        break;
                    }
                }
            }
            continue;
        }

        // Extract the type name
        let type_name = extract_type_name(content, &child);
        if !type_name.is_empty() {
            let rel = if is_interface_name(&type_name) {
                "implements".to_string()
            } else {
                "extends".to_string()
            };
            parents.push((type_name, rel));
        }
    }
    parents
}

/// Extract a clean type name from a type node, stripping generic parameters.
/// e.g. "IRepository<T>" -> "IRepository", "BaseEntity" -> "BaseEntity"
fn extract_type_name(content: &str, node: &tree_sitter::Node) -> String {
    match node.kind() {
        "identifier" => node_text(content, node).to_string(),
        "qualified_name" => node_text(content, node).to_string(),
        "generic_name" => {
            // For generic_name, just take the identifier part (first child)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    return node_text(content, &child).to_string();
                }
            }
            node_text(content, node).to_string()
        }
        "predefined_type" => node_text(content, node).to_string(),
        _ => {
            // For other node types, try to get the text directly
            let text = node_text(content, node).trim().to_string();
            // Strip generic parameters if present
            if let Some(idx) = text.find('<') {
                text[..idx].to_string()
            } else {
                text
            }
        }
    }
}

/// Extract variable names from a field_declaration or event_field_declaration node.
/// These nodes contain: modifiers, variable_declaration { type, variable_declarator { name } }
fn extract_field_info(content: &str, node: &tree_sitter::Node) -> Vec<(String, usize, bool)> {
    let mut results = Vec::new();
    let mut has_const = false;

    // Check modifiers for const
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            let mod_text = node_text(content, &child);
            if mod_text == "const" {
                has_const = true;
            }
        }
        if child.kind() == "variable_declaration" {
            let mut inner_cursor = child.walk();
            for var_child in child.children(&mut inner_cursor) {
                if var_child.kind() == "variable_declarator" {
                    // Get the name field
                    if let Some(name_node) = var_child.child_by_field_name("name") {
                        let name = node_text(content, &name_node).to_string();
                        let line = node_line(&name_node);
                        results.push((name, line, has_const));
                    }
                }
            }
        }
    }
    results
}

/// Extract event field variable names from an event_field_declaration node.
fn extract_event_field_names(content: &str, node: &tree_sitter::Node) -> Vec<(String, usize)> {
    let mut results = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declaration" {
            let mut inner_cursor = child.walk();
            for var_child in child.children(&mut inner_cursor) {
                if var_child.kind() == "variable_declarator" {
                    if let Some(name_node) = var_child.child_by_field_name("name") {
                        let name = node_text(content, &name_node).to_string();
                        let line = node_line(&name_node);
                        results.push((name, line));
                    }
                }
            }
        }
    }
    results
}

/// Extract the name from a using_directive node.
/// Handles both `using Foo.Bar;` and `using Alias = Foo.Bar;`
fn extract_using_name(content: &str, node: &tree_sitter::Node) -> Option<(String, usize)> {
    let mut cursor = node.walk();
    let line = node_line(node);

    // Walk children to find the name/qualified_name
    for child in node.children(&mut cursor) {
        match child.kind() {
            "qualified_name" | "identifier" => {
                let name = node_text(content, &child).to_string();
                return Some((name, line));
            }
            _ => {}
        }
    }
    None
}

/// Comments and string and character literals; the holes of an interpolated string are code.
static NON_CODE: super::NonCode = super::NonCode {
    language: &CSHARP_LANGUAGE,
    prose: &["comment"],
    strings: &[
        "string_literal",
        "verbatim_string_literal",
        "raw_string_literal",
        "character_literal",
        "interpolated_string_expression",
    ],
    code: &["interpolation"],
    keep: super::keep_no_string,
    declared: super::declares_nothing,
};

impl LanguageParser for CSharpParser {
    fn non_code(&self) -> Option<&'static super::NonCode> {
        Some(&NON_CODE)
    }

    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &CSHARP_LANGUAGE)?;
        let mut symbols = Vec::new();
        let mut cursor = QueryCursor::new();
        let query = &*CSHARP_QUERY;

        // Build capture name -> index map
        let capture_names = query.capture_names();
        let idx = |name: &str| -> Option<u32> {
            capture_names
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };

        let idx_namespace_name = idx("namespace_name");
        let idx_using_dir = idx("using_dir");
        let idx_class_name = idx("class_name");
        let idx_class_decl = idx("class_decl");
        let idx_interface_name = idx("interface_name");
        let idx_interface_decl = idx("interface_decl");
        let idx_struct_name = idx("struct_name");
        let idx_record_name = idx("record_name");
        let idx_record_decl = idx("record_decl");
        let idx_enum_name = idx("enum_name");
        let idx_method_name = idx("method_name");
        let idx_constructor_name = idx("constructor_name");
        let idx_property_name = idx("property_name");
        let idx_field_decl = idx("field_decl");
        let idx_event_field_decl = idx("event_field_decl");
        let idx_event_name = idx("event_name");
        let idx_delegate_name = idx("delegate_name");
        let idx_attr_name = idx("attr_name");
        let idx_definition = idx("definition");

        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());

        while let Some(m) = matches.next() {
            let end_line = find_capture(m, idx_definition).map(|c| text_end_line(content, &c.node));

            // Namespace
            if let Some(cap) = find_capture(m, idx_namespace_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                // `namespace A.B;` scopes the rest of the file, but the grammar
                // ends the node at the semicolon.
                let end_line = match cap.node.parent() {
                    Some(decl) if decl.kind() == "file_scoped_namespace_declaration" => decl
                        .parent()
                        .map(|unit| text_end_line(content, &unit))
                        .or(end_line),
                    _ => end_line,
                };
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Package,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Using directive
            if let Some(cap) = find_capture(m, idx_using_dir) {
                if let Some((name, line)) = extract_using_name(content, &cap.node) {
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Import,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Class
            if let Some(cap) = find_capture(m, idx_class_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let parents = find_capture(m, idx_class_decl)
                    .and_then(|dc| find_base_list_child(&dc.node))
                    .map(|bl| parse_base_list(content, &bl))
                    .unwrap_or_default();
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    line,
                    signature: signature_line(content, line),
                    parents,
                    end_line,
                });
                continue;
            }

            // Interface
            if let Some(cap) = find_capture(m, idx_interface_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let parents = find_capture(m, idx_interface_decl)
                    .and_then(|dc| find_base_list_child(&dc.node))
                    .map(|bl| parse_base_list(content, &bl))
                    .unwrap_or_default();
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    line,
                    signature: signature_line(content, line),
                    parents,
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
                    kind: SymbolKind::Class, // Struct -> Class
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Record
            if let Some(cap) = find_capture(m, idx_record_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                let parents = find_capture(m, idx_record_decl)
                    .and_then(|dc| find_base_list_child(&dc.node))
                    .map(|bl| parse_base_list(content, &bl))
                    .unwrap_or_default();
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class, // Record -> Class
                    line,
                    signature: signature_line(content, line),
                    parents,
                    end_line,
                });
                continue;
            }

            // Enum
            if let Some(cap) = find_capture(m, idx_enum_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Method
            if let Some(cap) = find_capture(m, idx_method_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Constructor
            if let Some(cap) = find_capture(m, idx_constructor_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Property
            if let Some(cap) = find_capture(m, idx_property_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property,
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Field declaration (may contain const)
            if let Some(cap) = find_capture(m, idx_field_decl) {
                let fields = extract_field_info(content, &cap.node);
                for (name, line, is_const) in fields {
                    let kind = if is_const {
                        SymbolKind::Constant
                    } else {
                        SymbolKind::Property
                    };
                    symbols.push(ParsedSymbol {
                        name,
                        kind,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Event field declaration
            if let Some(cap) = find_capture(m, idx_event_field_decl) {
                let events = extract_event_field_names(content, &cap.node);
                for (name, line) in events {
                    symbols.push(ParsedSymbol {
                        name,
                        kind: SymbolKind::Property, // Event -> Property
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }

            // Event declaration (with accessors)
            if let Some(cap) = find_capture(m, idx_event_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Property, // Event -> Property
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Delegate
            if let Some(cap) = find_capture(m, idx_delegate_name) {
                let name = node_text(content, &cap.node);
                let line = node_line(&cap.node);
                symbols.push(ParsedSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::TypeAlias, // Delegate -> TypeAlias
                    line,
                    signature: signature_line(content, line),
                    parents: vec![],
                    end_line,
                });
                continue;
            }

            // Attribute
            if let Some(cap) = find_capture(m, idx_attr_name) {
                let attr_name = node_text(content, &cap.node);
                // Extract just the simple name (last component of qualified name)
                let simple_name = attr_name.rsplit('.').next().unwrap_or(attr_name);
                let line = node_line(&cap.node);
                if is_significant_attr(simple_name) {
                    symbols.push(ParsedSymbol {
                        name: format!("[{}]", simple_name),
                        kind: SymbolKind::Annotation,
                        line,
                        signature: signature_line(content, line),
                        parents: vec![],
                        end_line,
                    });
                }
                continue;
            }
        }

        Ok(symbols)
    }
}

/// Find a base_list child node within a declaration node
fn find_base_list_child<'a>(node: &'a tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "base_list" {
            return Some(child);
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
    fn test_parse_namespace() {
        let content = r#"namespace MyApp.Models
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp.Models" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_file_scoped_namespace() {
        let content = "namespace MyApp.Services;\n";
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp.Services" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_using() {
        let content = r#"using System;
using System.Collections.Generic;
using System.Linq;
using MyApp.Models;
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "System" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "System.Collections.Generic" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "System.Linq" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp.Models" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_class() {
        let content = r#"namespace MyApp
{
    public class User : BaseEntity, IDisposable
    {
    }

    public abstract class BaseEntity
    {
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp" && s.kind == SymbolKind::Package));
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "BaseEntity" && s.kind == SymbolKind::Class));
        // Check parents
        let user = symbols
            .iter()
            .find(|s| s.name == "User" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(user
            .parents
            .iter()
            .any(|(p, k)| p == "BaseEntity" && k == "extends"));
        assert!(user
            .parents
            .iter()
            .any(|(p, k)| p == "IDisposable" && k == "implements"));
    }

    #[test]
    fn test_parse_generic_class() {
        let content = r#"public class Repository<T> : IRepository<T> where T : class
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Repository" && s.kind == SymbolKind::Class));
        let repo = symbols.iter().find(|s| s.name == "Repository").unwrap();
        assert!(repo
            .parents
            .iter()
            .any(|(p, k)| p == "IRepository" && k == "implements"));
    }

    #[test]
    fn test_parse_interface() {
        let content = r#"public interface IRepository<T> : IDisposable
{
    T GetById(int id);
    void Save(T entity);
}

public interface IUserRepository : IRepository<User>
{
    User FindByEmail(string email);
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "IRepository" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "IUserRepository" && s.kind == SymbolKind::Interface));
        // Interface methods
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetById" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Save" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "FindByEmail" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_struct() {
        let content = r#"public struct Point
{
    public int X;
    public int Y;
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Point" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_record() {
        let content = r#"public record Person(string FirstName, string LastName);

public record Employee(string FirstName, string LastName, string Department) : Person(FirstName, LastName);

public record struct Point(int X, int Y);
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Person" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Employee" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Point" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_enum() {
        let content = r#"public enum Status
{
    Active,
    Inactive,
    Pending
}

internal enum Priority
{
    Low = 1,
    Medium = 2,
    High = 3
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Priority" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_methods() {
        let content = r#"public class UserService
{
    public async Task<User> GetUserAsync(int id)
    {
        return null;
    }

    public void SaveUser(User user)
    {
    }

    private static bool ValidateEmail(string email)
    {
        return false;
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetUserAsync" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "SaveUser" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ValidateEmail" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_constructor() {
        let content = r#"public class UserService
{
    private readonly ILogger _logger;

    public UserService(ILogger logger)
    {
        _logger = logger;
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserService" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_properties() {
        let content = r#"public class Config
{
    public string Name { get; set; }
    public int MaxRetries { get; private set; }
    public required string ApiKey { get; init; }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Name" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MaxRetries" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ApiKey" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_fields() {
        let content = r#"public class Config
{
    private readonly ILogger _logger;
    private static string _connectionString;
    public int Count;
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "_logger" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "_connectionString" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Count" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_const() {
        let content = r#"public class Config
{
    public const int MAX_RETRIES = 5;
    public const string DEFAULT_NAME = "test";
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));
        assert!(symbols
            .iter()
            .any(|s| s.name == "DEFAULT_NAME" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_delegate() {
        let content = r#"public delegate void EventHandler(object sender, EventArgs e);
public delegate Task<T> AsyncHandler<T>(CancellationToken token);
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "EventHandler" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols
            .iter()
            .any(|s| s.name == "AsyncHandler" && s.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_parse_event_field() {
        let content = r#"public class Publisher
{
    public event EventHandler OnDataReceived;
    public event Action<string> OnMessage;
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "OnDataReceived" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "OnMessage" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_event_with_accessors() {
        let content = r#"public class Publisher
{
    public event EventHandler OnData
    {
        add { }
        remove { }
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "OnData" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_attributes() {
        let content = r#"[ApiController]
[Route("api/[controller]")]
public class UsersController : ControllerBase
{
    [HttpGet]
    public IActionResult GetAll()
    {
        return Ok();
    }

    [Authorize]
    [HttpPost]
    public IActionResult Create(UserDto user)
    {
        return Created();
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "[ApiController]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[Route]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[HttpGet]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[Authorize]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[HttpPost]" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_parse_test_attributes() {
        let content = r#"public class UserTests
{
    [Fact]
    public void TestCreate()
    {
    }

    [Theory]
    public void TestValidate(string input)
    {
    }

    [Test]
    public void NUnitTest()
    {
    }

    [TestMethod]
    public void MSTestMethod()
    {
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "[Fact]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[Theory]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[Test]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[TestMethod]" && s.kind == SymbolKind::Annotation));
    }

    #[test]
    fn test_comments_ignored() {
        let content = r#"// class FakeClass {}
class RealClass
{
}
/* interface IFake {} */
interface IReal
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "RealClass"));
        assert!(!symbols.iter().any(|s| s.name == "FakeClass"));
        assert!(symbols.iter().any(|s| s.name == "IReal"));
        assert!(!symbols.iter().any(|s| s.name == "IFake"));
    }

    #[test]
    fn test_class_with_multiple_interfaces() {
        let content = r#"public class Service : IService, IDisposable, IAsyncDisposable
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        let svc = symbols
            .iter()
            .find(|s| s.name == "Service" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(svc
            .parents
            .iter()
            .any(|(p, k)| p == "IService" && k == "implements"));
        assert!(svc
            .parents
            .iter()
            .any(|(p, k)| p == "IDisposable" && k == "implements"));
        assert!(svc
            .parents
            .iter()
            .any(|(p, k)| p == "IAsyncDisposable" && k == "implements"));
    }

    #[test]
    fn test_class_extends_and_implements() {
        let content = r#"public class UserService : BaseService, IUserService
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        let svc = symbols.iter().find(|s| s.name == "UserService").unwrap();
        assert!(svc
            .parents
            .iter()
            .any(|(p, k)| p == "BaseService" && k == "extends"));
        assert!(svc
            .parents
            .iter()
            .any(|(p, k)| p == "IUserService" && k == "implements"));
    }

    #[test]
    fn test_parse_complete_file() {
        let content = r#"using System;
using System.Collections.Generic;

namespace MyApp.Services
{
    [ApiController]
    public class UserController : ControllerBase, IDisposable
    {
        private readonly ILogger _logger;
        public const int MAX_RETRIES = 3;

        public string Name { get; set; }

        public UserController(ILogger logger)
        {
            _logger = logger;
        }

        [HttpGet]
        public async Task<User> GetUser(int id)
        {
            return null;
        }

        public event EventHandler OnUserCreated;
    }

    public interface IUserService : IDisposable
    {
        User GetById(int id);
    }

    public enum UserStatus
    {
        Active,
        Inactive
    }

    public record UserDto(string Name, string Email);

    public delegate void UserHandler(User user);

    public struct Coordinate
    {
        public double Lat;
        public double Lng;
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();

        // Imports
        assert!(symbols
            .iter()
            .any(|s| s.name == "System" && s.kind == SymbolKind::Import));
        assert!(symbols
            .iter()
            .any(|s| s.name == "System.Collections.Generic" && s.kind == SymbolKind::Import));

        // Namespace
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp.Services" && s.kind == SymbolKind::Package));

        // Attributes
        assert!(symbols
            .iter()
            .any(|s| s.name == "[ApiController]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "[HttpGet]" && s.kind == SymbolKind::Annotation));

        // Class with parents
        let ctrl = symbols
            .iter()
            .find(|s| s.name == "UserController" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(ctrl
            .parents
            .iter()
            .any(|(p, k)| p == "ControllerBase" && k == "extends"));
        assert!(ctrl
            .parents
            .iter()
            .any(|(p, k)| p == "IDisposable" && k == "implements"));

        // Fields and constants
        assert!(symbols
            .iter()
            .any(|s| s.name == "_logger" && s.kind == SymbolKind::Property));
        assert!(symbols
            .iter()
            .any(|s| s.name == "MAX_RETRIES" && s.kind == SymbolKind::Constant));

        // Properties
        assert!(symbols
            .iter()
            .any(|s| s.name == "Name" && s.kind == SymbolKind::Property));

        // Constructor
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserController" && s.kind == SymbolKind::Function));

        // Methods
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetUser" && s.kind == SymbolKind::Function));

        // Events
        assert!(symbols
            .iter()
            .any(|s| s.name == "OnUserCreated" && s.kind == SymbolKind::Property));

        // Interface
        let iface = symbols
            .iter()
            .find(|s| s.name == "IUserService" && s.kind == SymbolKind::Interface)
            .unwrap();
        assert!(iface
            .parents
            .iter()
            .any(|(p, k)| p == "IDisposable" && k == "implements"));

        // Enum
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserStatus" && s.kind == SymbolKind::Enum));

        // Record
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserDto" && s.kind == SymbolKind::Class));

        // Delegate
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserHandler" && s.kind == SymbolKind::TypeAlias));

        // Struct
        assert!(symbols
            .iter()
            .any(|s| s.name == "Coordinate" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_non_significant_attributes_ignored() {
        let content = r#"[SomeCustomAttribute]
public class Foo
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        // Custom attributes should NOT be tracked
        assert!(!symbols.iter().any(|s| s.name == "[SomeCustomAttribute]"));
        // But the class should be
        assert!(symbols
            .iter()
            .any(|s| s.name == "Foo" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_sealed_partial_class() {
        let content = r#"public sealed partial class AppSettings
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "AppSettings" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_static_using() {
        let content = "using static System.Math;\n";
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "System.Math" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_abstract_method() {
        let content = r#"public abstract class Base
{
    public abstract void Process();
    public virtual string GetName()
    {
        return "";
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Base" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Process" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetName" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_record_with_base() {
        let content = r#"public record Employee(string Name) : Person(Name), IComparable
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        let emp = symbols
            .iter()
            .find(|s| s.name == "Employee" && s.kind == SymbolKind::Class)
            .unwrap();
        assert!(emp
            .parents
            .iter()
            .any(|(p, k)| p == "Person" && k == "extends"));
        assert!(emp
            .parents
            .iter()
            .any(|(p, k)| p == "IComparable" && k == "implements"));
    }

    #[test]
    fn test_interface_extends_interface() {
        let content = r#"public interface IAdvanced : IBasic, IExtended
{
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        let iface = symbols
            .iter()
            .find(|s| s.name == "IAdvanced" && s.kind == SymbolKind::Interface)
            .unwrap();
        assert!(iface
            .parents
            .iter()
            .any(|(p, k)| p == "IBasic" && k == "implements"));
        assert!(iface
            .parents
            .iter()
            .any(|(p, k)| p == "IExtended" && k == "implements"));
    }

    #[test]
    fn test_obsolete_attribute() {
        let content = r#"public class MyClass
{
    [Obsolete("Use NewMethod instead")]
    public void OldMethod()
    {
    }
}
"#;
        let symbols = CSHARP_PARSER.parse_symbols(content).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "[Obsolete]" && s.kind == SymbolKind::Annotation));
        assert!(symbols
            .iter()
            .any(|s| s.name == "OldMethod" && s.kind == SymbolKind::Function));
    }
}
