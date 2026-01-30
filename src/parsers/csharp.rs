use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse C# source file and extract symbols
pub fn parse_csharp_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Namespace: namespace Name.Space { or namespace Name.Space;
    static NAMESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*namespace\s+([A-Za-z_][A-Za-z0-9_\.]*)"

    ).unwrap());

    let namespace_re = &*NAMESPACE_RE;

    // Class: class ClassName : Base, IInterface
    static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|abstract|sealed|static|partial)\s+)*class\s+([A-Z][A-Za-z0-9_]*)(?:\s*<[^>]*>)?(?:\s*:\s*([A-Za-z0-9_<>,\s]+))?"

    ).unwrap());

    let class_re = &*CLASS_RE;

    // Interface: interface IName : IBase
    static INTERFACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal)\s+)*interface\s+(I[A-Z][A-Za-z0-9_]*)(?:\s*<[^>]*>)?(?:\s*:\s*([A-Za-z0-9_<>,\s]+))?"

    ).unwrap());

    let interface_re = &*INTERFACE_RE;

    // Struct: struct Name
    static STRUCT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|readonly)\s+)*struct\s+([A-Z][A-Za-z0-9_]*)(?:\s*<[^>]*>)?"

    ).unwrap());

    let struct_re = &*STRUCT_RE;

    // Record: record Name or record class Name or record struct Name
    static RECORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|sealed|abstract)\s+)*record\s+(?:class\s+|struct\s+)?([A-Z][A-Za-z0-9_]*)(?:\s*<[^>]*>)?(?:\s*\([^)]*\))?(?:\s*:\s*([A-Za-z0-9_<>,\s]+))?"

    ).unwrap());

    let record_re = &*RECORD_RE;

    // Enum: enum Name
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal)\s+)*enum\s+([A-Z][A-Za-z0-9_]*)"

    ).unwrap());

    let enum_re = &*ENUM_RE;

    // Method: returnType MethodName(params)
    static METHOD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|static|virtual|override|abstract|async|sealed|new|extern)\s+)*(?:[A-Za-z_][A-Za-z0-9_<>\[\]?,\s]*\s+)?([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*\([^)]*\)\s*(?:where\s+[^{;]+)?[{;]"

    ).unwrap());

    let method_re = &*METHOD_RE;

    // Property: Type PropertyName { get; set; }
    static PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|static|virtual|override|abstract|new)\s+)*(?:required\s+)?(?:[A-Za-z_][A-Za-z0-9_<>\[\]?,\s]*)\s+([A-Z][A-Za-z0-9_]*)\s*\{"

    ).unwrap());

    let property_re = &*PROPERTY_RE;

    // Field: private readonly Type _fieldName;
    static FIELD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|static|readonly|const|volatile)\s+)+(?:[A-Za-z_][A-Za-z0-9_<>\[\]?,\s]*)\s+(_[a-z][A-Za-z0-9_]*)\s*[;=]"

    ).unwrap());

    let field_re = &*FIELD_RE;

    // Constant: const Type CONSTANT_NAME = value;
    static CONST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal)\s+)*const\s+(?:[A-Za-z_][A-Za-z0-9_<>\[\]?,\s]*)\s+([A-Z][A-Z0-9_]*)\s*="

    ).unwrap());

    let const_re = &*CONST_RE;

    // Delegate: delegate returnType DelegateName(params);
    static DELEGATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal)\s+)*delegate\s+(?:[A-Za-z_][A-Za-z0-9_<>\[\]?,\s]*)\s+([A-Z][A-Za-z0-9_]*)(?:\s*<[^>]*>)?\s*\("

    ).unwrap());

    let delegate_re = &*DELEGATE_RE;

    // Event: event EventHandler EventName;
    static EVENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*(?:(?:public|private|protected|internal|static|virtual|override|abstract)\s+)*event\s+(?:[A-Za-z_][A-Za-z0-9_<>\[\]?,\s]*)\s+([A-Z][A-Za-z0-9_]*)\s*[;{]"

    ).unwrap());

    let event_re = &*EVENT_RE;

    // Using: using Namespace;
    static USING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*using\s+(?:static\s+)?([A-Za-z_][A-Za-z0-9_\.]*)\s*;"

    ).unwrap());

    let using_re = &*USING_RE;

    // Attribute: [AttributeName] or [AttributeName(params)]
    static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[ \t]*\[([A-Za-z_][A-Za-z0-9_]*)(?:\([^\]]*\))?\]"

    ).unwrap());

    let attr_re = &*ATTR_RE;

    let lines: Vec<&str> = content.lines().collect();

    // Parse namespaces
    for cap in namespace_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Package,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse classes
    for cap in class_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let parents_str = cap.get(2).map(|m| m.as_str());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let parents: Vec<(String, String)> = parents_str
            .map(|s| {
                s.split(',')
                    .map(|p| {
                        let p = p.trim().to_string();
                        let kind = if p.starts_with('I') && p.chars().nth(1).map(|c| c.is_uppercase()).unwrap_or(false) {
                            "implements".to_string()
                        } else {
                            "extends".to_string()
                        };
                        (p, kind)
                    })
                    .filter(|(p, _)| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents,
        });
    }

    // Parse interfaces
    for cap in interface_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let parents_str = cap.get(2).map(|m| m.as_str());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let parents: Vec<(String, String)> = parents_str
            .map(|s| {
                s.split(',')
                    .map(|p| (p.trim().to_string(), "extends".to_string()))
                    .filter(|(p, _)| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Interface,
            line,
            signature: line_text.trim().to_string(),
            parents,
        });
    }

    // Parse structs
    for cap in struct_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class, // Struct -> Class
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse records
    for cap in record_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let parents_str = cap.get(2).map(|m| m.as_str());
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        let parents: Vec<(String, String)> = parents_str
            .map(|s| {
                s.split(',')
                    .map(|p| {
                        let p = p.trim().to_string();
                        let kind = if p.starts_with('I') && p.chars().nth(1).map(|c| c.is_uppercase()).unwrap_or(false) {
                            "implements".to_string()
                        } else {
                            "extends".to_string()
                        };
                        (p, kind)
                    })
                    .filter(|(p, _)| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class, // Record -> Class
            line,
            signature: line_text.trim().to_string(),
            parents,
        });
    }

    // Parse enums
    for cap in enum_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Enum,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse methods
    for cap in method_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if this looks like a class/interface/struct definition (already handled)
        if line_text.contains("class ") || line_text.contains("interface ")
            || line_text.contains("struct ") || line_text.contains("record ")
            || line_text.contains("enum ") {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse properties
    for cap in property_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if this looks like a method
        if line_text.contains('(') {
            continue;
        }

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Property,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse fields
    for cap in field_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Property,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse constants
    for cap in const_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Constant,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse delegates
    for cap in delegate_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::TypeAlias, // Delegate -> TypeAlias
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse events
    for cap in event_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Property, // Event -> Property
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse using statements
    for cap in using_re.captures_iter(content) {
        let ns = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: ns.to_string(),
            kind: SymbolKind::Import,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse significant attributes
    for cap in attr_re.captures_iter(content) {
        let attr_name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Only track significant attributes
        if matches!(attr_name,
            "Serializable" | "DataContract" | "DataMember" |
            "JsonProperty" | "JsonIgnore" | "Required" |
            "Authorize" | "AllowAnonymous" | "HttpGet" | "HttpPost" | "HttpPut" | "HttpDelete" |
            "Route" | "ApiController" | "Controller" |
            "Test" | "TestMethod" | "Fact" | "Theory" |
            "SerializeField" | "Header" | "Tooltip" | "Range" |
            "DllImport" | "StructLayout" | "MarshalAs" |
            "Obsolete" | "Conditional" | "DebuggerDisplay"
        ) {
            symbols.push(ParsedSymbol {
                name: format!("[{}]", attr_name),
                kind: SymbolKind::Annotation,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    Ok(symbols)
}

fn find_line_number(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_class() {
        let content = r#"
namespace MyApp
{
    public class User : BaseEntity, IDisposable
    {
        public string Name { get; set; }
    }

    public abstract class BaseEntity
    {
        public int Id { get; set; }
    }
}
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MyApp" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "BaseEntity" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "User" && s.parents.iter().any(|(p, k)| p == "BaseEntity" && k == "extends")));
        assert!(symbols.iter().any(|s| s.name == "User" && s.parents.iter().any(|(p, k)| p == "IDisposable" && k == "implements")));
    }

    #[test]
    fn test_parse_interface() {
        let content = r#"
public interface IRepository<T> : IDisposable
{
    T GetById(int id);
    void Save(T entity);
}

public interface IUserRepository : IRepository<User>
{
    User FindByEmail(string email);
}
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "IRepository" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "IUserRepository" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_record() {
        let content = r#"
public record Person(string FirstName, string LastName);

public record Employee(string FirstName, string LastName, string Department) : Person(FirstName, LastName);

public record struct Point(int X, int Y);
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Person" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Employee" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Point" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_methods() {
        let content = r#"
public class UserService
{
    public async Task<User> GetUserAsync(int id)
    {
        return await _repository.GetByIdAsync(id);
    }

    public void SaveUser(User user)
    {
        _repository.Save(user);
    }

    private static bool ValidateEmail(string email)
    {
        return email.Contains("@");
    }
}
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "GetUserAsync" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "SaveUser" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "ValidateEmail" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_properties_fields() {
        let content = r#"
public class Config
{
    private readonly ILogger _logger;
    private static string _connectionString;

    public string Name { get; set; }
    public int MaxRetries { get; private set; }
    public required string ApiKey { get; init; }
}
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "_logger" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "Name" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "MaxRetries" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "ApiKey" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_enum() {
        let content = r#"
public enum Status
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
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols.iter().any(|s| s.name == "Priority" && s.kind == SymbolKind::Enum));
    }

    #[test]
    fn test_parse_using() {
        let content = r#"
using System;
using System.Collections.Generic;
using System.Linq;
using static System.Math;
using MyApp.Models;
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "System" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "System.Collections.Generic" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "System.Math" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_attributes() {
        let content = r#"
[ApiController]
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
    public IActionResult Create([Required] UserDto user)
    {
        return Created();
    }
}
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "[ApiController]" && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(|s| s.name == "[HttpGet]" && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(|s| s.name == "[Authorize]" && s.kind == SymbolKind::Annotation));
        assert!(symbols.iter().any(|s| s.name == "[HttpPost]" && s.kind == SymbolKind::Annotation));
        // Note: [Required] is a parameter-level attribute (inline), not tracked by default
    }

    #[test]
    fn test_parse_delegate_event() {
        let content = r#"
public delegate void EventHandler(object sender, EventArgs e);
public delegate Task<T> AsyncHandler<T>(CancellationToken token);

public class Publisher
{
    public event EventHandler OnDataReceived;
    public event Action<string> OnMessage;
}
"#;
        let symbols = parse_csharp_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "EventHandler" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "AsyncHandler" && s.kind == SymbolKind::TypeAlias));
        assert!(symbols.iter().any(|s| s.name == "OnDataReceived" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "OnMessage" && s.kind == SymbolKind::Property));
    }
}
