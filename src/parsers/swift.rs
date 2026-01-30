//! Swift symbol parser
//!
//! Parses Swift source files (.swift) to extract:
//! - Classes, Structs, Enums
//! - Protocols
//! - Actors
//! - Extensions
//! - Functions and Init
//! - Properties
//! - Type aliases

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Swift source code and extract symbols
pub fn parse_swift_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Swift class: public/private/internal/final class ClassName: Parent, Protocol
    static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|open|final)\s+)*class\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"

    ).unwrap());

    let class_re = &*CLASS_RE;

    // Swift struct
    static STRUCT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate)\s+)*struct\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"

    ).unwrap());

    let struct_re = &*STRUCT_RE;

    // Swift enum
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate)\s+)*enum\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"

    ).unwrap());

    let enum_re = &*ENUM_RE;

    // Swift protocol
    static PROTOCOL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate)\s+)*protocol\s+(\w+)(?:\s*:\s*([^{]+))?"

    ).unwrap());

    let protocol_re = &*PROTOCOL_RE;

    // Swift actor
    static ACTOR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|final)\s+)*actor\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"

    ).unwrap());

    let actor_re = &*ACTOR_RE;

    // Swift extension
    static EXTENSION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|internal|fileprivate)\s+)?extension\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"

    ).unwrap());

    let extension_re = &*EXTENSION_RE;

    // Swift func (including async/throws)
    static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|open|final|override|static|class|mutating)\s+)*func\s+(\w+)\s*(?:<[^>]*>)?\s*\([^)]*\)"

    ).unwrap());

    let func_re = &*FUNC_RE;

    // Swift init
    static INIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|internal|fileprivate|override|convenience|required)\s+)*init\s*(?:\?|!)?\s*\("

    ).unwrap());

    let init_re = &*INIT_RE;

    // Swift var/let properties
    static PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|static|class|lazy|weak|unowned)\s+)*(var|let)\s+(\w+)\s*:"

    ).unwrap());

    let property_re = &*PROPERTY_RE;

    // Swift typealias
    static TYPEALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|internal|fileprivate)\s+)?typealias\s+(\w+)(?:\s*<[^>]*>)?\s*="

    ).unwrap());

    let typealias_re = &*TYPEALIAS_RE;

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;

        // Classes
        if let Some(caps) = class_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(4).map(|m| m.as_str().trim());
            let parents = parse_swift_parents(parents_str);

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Class,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Structs
        if let Some(caps) = struct_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(4).map(|m| m.as_str().trim());
            let parents = parse_swift_parents(parents_str);

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Class, // Use Class for struct too (same semantics for search)
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Enums
        if let Some(caps) = enum_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(4).map(|m| m.as_str().trim());
            let parents = parse_swift_parents(parents_str);

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Enum,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Protocols (like interfaces)
        if let Some(caps) = protocol_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(4).map(|m| m.as_str().trim());
            let parents = parse_swift_parents(parents_str);

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Interface, // Protocol ~ Interface
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Actors
        if let Some(caps) = actor_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(4).map(|m| m.as_str().trim());
            let parents = parse_swift_parents(parents_str);

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Class, // Actor ~ Class
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Extensions (track what type is being extended)
        if let Some(caps) = extension_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            let extended_name = format!("{}+Extension", name);

            symbols.push(ParsedSymbol {
                name: extended_name,
                kind: SymbolKind::Object, // Use Object for extensions
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![(name, "extends".to_string())],
            });
        }

        // Functions
        if let Some(caps) = func_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Function,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Init (constructors)
        if init_re.is_match(line) {
            symbols.push(ParsedSymbol {
                name: "init".to_string(),
                kind: SymbolKind::Function,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Properties
        if let Some(caps) = property_re.captures(line) {
            let name = caps.get(4).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Property,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Type aliases
        if let Some(caps) = typealias_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::TypeAlias,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }
    }

    Ok(symbols)
}

/// Parse Swift parent types (protocols, base class)
pub fn parse_swift_parents(parents_str: Option<&str>) -> Vec<(String, String)> {
    let mut parents = Vec::new();

    if let Some(ps) = parents_str {
        for parent in ps.split(',') {
            let parent = parent.trim().split('<').next().unwrap_or("").trim();
            let parent = parent.split("where").next().unwrap_or(parent).trim();
            if !parent.is_empty() {
                // In Swift, first parent could be class (extends), rest are protocols (implements)
                let kind = if parents.is_empty() {
                    "extends" // Could be class or protocol
                } else {
                    "implements"
                };
                parents.push((parent.to_string(), kind.to_string()));
            }
        }
    }

    parents
}
