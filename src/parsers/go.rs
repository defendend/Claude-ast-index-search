use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Go source file and extract symbols
pub fn parse_go_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Package declaration: package name
    static PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^package\s+([a-z][a-z0-9_]*)").unwrap());

    let package_re = &*PACKAGE_RE;

    // Import: import "module" or import ( "module1" "module2" )
    static IMPORT_SINGLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^import\s+"([^"]+)""#).unwrap());

    let import_single_re = &*IMPORT_SINGLE_RE;
    static IMPORT_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)import\s*\(\s*([^)]+)\)"#).unwrap());

    let import_block_re = &*IMPORT_BLOCK_RE;
    static IMPORT_LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:([a-zA-Z_][a-zA-Z0-9_]*)\s+)?"([^"]+)""#).unwrap());

    let import_line_re = &*IMPORT_LINE_RE;

    // Type struct: type Name struct { ... }
    static STRUCT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^type\s+([A-Z][a-zA-Z0-9_]*)\s+struct\s*\{").unwrap());

    let struct_re = &*STRUCT_RE;

    // Type interface: type Name interface { ... }
    static INTERFACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^type\s+([A-Z][a-zA-Z0-9_]*)\s+interface\s*\{").unwrap());

    let interface_re = &*INTERFACE_RE;

    // Type alias: type Name = OtherType or type Name OtherType
    static TYPE_ALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^type\s+([A-Z][a-zA-Z0-9_]*)\s+(?:=\s*)?([a-zA-Z][a-zA-Z0-9_\.\[\]]*)\s*$").unwrap());

    let type_alias_re = &*TYPE_ALIAS_RE;

    // Function: func Name(...) ... { or func (r *Receiver) Name(...) ... {
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^func\s+(?:\([^)]+\)\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)").unwrap());

    let func_re = &*FUNC_RE;

    // Method with receiver: func (r *Type) Method(...)
    static METHOD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^func\s+\(\s*\w+\s+\*?([A-Z][a-zA-Z0-9_]*)\s*\)\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap());

    let method_re = &*METHOD_RE;

    // Const declaration: const Name = value or const ( Name = value )
    static CONST_SINGLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^const\s+([A-Z][A-Za-z0-9_]*)\s*(?:=|[a-zA-Z])").unwrap());

    let const_single_re = &*CONST_SINGLE_RE;
    static CONST_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)const\s*\(\s*([^)]+)\)").unwrap());

    let const_block_re = &*CONST_BLOCK_RE;
    static CONST_LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*(?:=|[a-zA-Z])").unwrap());

    let const_line_re = &*CONST_LINE_RE;

    // Var declaration at package level
    static VAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^var\s+([A-Z][a-zA-Z0-9_]*)\s+").unwrap());

    let var_re = &*VAR_RE;

    let lines: Vec<&str> = content.lines().collect();

    // Parse package
    if let Some(cap) = package_re.captures(content) {
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

    // Parse single imports
    for cap in import_single_re.captures_iter(content) {
        let path = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Extract package name from path
        let name = path.rsplit('/').next().unwrap_or(path);

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Import,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![(path.to_string(), "from".to_string())],
        });
    }

    // Parse import blocks
    for cap in import_block_re.captures_iter(content) {
        let block = cap.get(1).unwrap().as_str();
        let block_start = cap.get(1).unwrap().start();

        for line_cap in import_line_re.captures_iter(block) {
            let path = line_cap.get(2).unwrap().as_str();
            let alias = line_cap.get(1).map(|m| m.as_str());
            let match_start = block_start + line_cap.get(0).unwrap().start();
            let line = find_line_number(content, match_start);

            let name = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path));

            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Import,
                line,
                signature: format!("import \"{}\"", path),
                parents: vec![(path.to_string(), "from".to_string())],
            });
        }
    }

    // Parse structs
    for cap in struct_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Class,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse interfaces
    for cap in interface_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: name.to_string(),
            kind: SymbolKind::Interface,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![],
        });
    }

    // Parse type aliases (but not structs/interfaces which are handled above)
    for cap in type_alias_re.captures_iter(content) {
        let name = cap.get(1).unwrap().as_str();
        let target = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Skip if this is a struct or interface definition
        if target != "struct" && target != "interface" {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::TypeAlias,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![(target.to_string(), "alias".to_string())],
            });
        }
    }

    // Parse methods (with receiver) - capture the receiver type as parent
    for cap in method_re.captures_iter(content) {
        let receiver_type = cap.get(1).unwrap().as_str();
        let method_name = cap.get(2).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        symbols.push(ParsedSymbol {
            name: method_name.to_string(),
            kind: SymbolKind::Function,
            line,
            signature: line_text.trim().to_string(),
            parents: vec![(receiver_type.to_string(), "receiver".to_string())],
        });
    }

    // Parse standalone functions (not methods)
    for cap in func_re.captures_iter(content) {
        let full_match = cap.get(0).unwrap().as_str();
        // Skip if this is a method (has receiver in parentheses)
        if full_match.contains(") ") && full_match.starts_with("func (") {
            continue;
        }

        let name = cap.get(1).unwrap().as_str();
        let start = cap.get(0).unwrap().start();
        let line = find_line_number(content, start);
        let line_text = lines.get(line - 1).unwrap_or(&"");

        // Avoid duplicates from method_re
        if !symbols.iter().any(|s| s.name == name && s.line == line) {
            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Function,
                line,
                signature: line_text.trim().to_string(),
                parents: vec![],
            });
        }
    }

    // Parse single constants
    for cap in const_single_re.captures_iter(content) {
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

    // Parse const blocks
    for cap in const_block_re.captures_iter(content) {
        let block = cap.get(1).unwrap().as_str();
        let block_start = cap.get(1).unwrap().start();

        for line_cap in const_line_re.captures_iter(block) {
            let name = line_cap.get(1).unwrap().as_str();
            let match_start = block_start + line_cap.get(0).unwrap().start();
            let line = find_line_number(content, match_start);

            symbols.push(ParsedSymbol {
                name: name.to_string(),
                kind: SymbolKind::Constant,
                line,
                signature: format!("const {}", name),
                parents: vec![],
            });
        }
    }

    // Parse package-level vars (exported only - start with uppercase)
    for cap in var_re.captures_iter(content) {
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

    Ok(symbols)
}

fn find_line_number(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package() {
        let content = "package main\n";
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == SymbolKind::Package));
    }

    #[test]
    fn test_parse_struct() {
        let content = r#"
type DeleteAction struct {
    avaSrv AvatarsMDS
}
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "DeleteAction" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_parse_interface() {
        let content = r#"
type AvatarsMDS interface {
    Delete(ctx context.Context, groupID int, name string) error
}
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "AvatarsMDS" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_method() {
        let content = r#"
func (a *DeleteAction) Do(ctx context.Context, task *entities.TaskToProcess) error {
    return nil
}
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Do" && s.parents.iter().any(|(p, _)| p == "DeleteAction")));
    }

    #[test]
    fn test_parse_function() {
        let content = r#"
func NewDeleteAction(avaSrv *avatarsmds.Service) *DeleteAction {
    return &DeleteAction{}
}
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "NewDeleteAction" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_parse_import_block() {
        let content = r#"
package main

import (
    "fmt"
    "net/http"
    log "github.com/sirupsen/logrus"
)
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "fmt" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "http" && s.kind == SymbolKind::Import));
        assert!(symbols.iter().any(|s| s.name == "log" && s.kind == SymbolKind::Import));
    }

    #[test]
    fn test_parse_const_block() {
        let content = r#"
const (
    StatusActive = iota
    StatusDeleted int = 1
    MaxRetries int = 5
)
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "StatusActive" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "MaxRetries" && s.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_parse_interface_methods() {
        let content = r#"
type Repository interface {
    Get(ctx context.Context, id int64) (*Entity, error)
    Save(ctx context.Context, entity *Entity) error
}
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "Repository" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_parse_exported_vs_unexported() {
        let content = r#"
type PublicStruct struct {}
func PublicFunc() {}
func privateFunc() {}
var PublicVar string
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "PublicStruct"));
        assert!(symbols.iter().any(|s| s.name == "PublicFunc"));
        assert!(symbols.iter().any(|s| s.name == "privateFunc"));
        assert!(symbols.iter().any(|s| s.name == "PublicVar" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_parse_type_alias() {
        let content = r#"
type UserID int64
type Callback func(string) error
"#;
        let symbols = parse_go_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "UserID" && s.kind == SymbolKind::TypeAlias));
    }
}
