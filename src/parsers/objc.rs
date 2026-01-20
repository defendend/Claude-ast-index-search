//! Objective-C symbol parser
//!
//! Parses Objective-C source files (.m, .h) to extract:
//! - @interface declarations
//! - @protocol definitions
//! - @implementation
//! - Methods
//! - @property
//! - typedef

use anyhow::Result;
use regex::Regex;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Objective-C source code and extract symbols
pub fn parse_objc_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // ObjC @interface: @interface ClassName : SuperClass <Protocol1, Protocol2>
    let interface_re = Regex::new(
        r"(?m)^[\s]*@interface\s+(\w+)(?:\s*\([^)]*\))?(?:\s*:\s*(\w+))?(?:\s*<([^>]+)>)?"
    )?;

    // ObjC @protocol definition
    let protocol_re = Regex::new(
        r"(?m)^[\s]*@protocol\s+(\w+)(?:\s*<([^>]+)>)?"
    )?;

    // ObjC @implementation
    let impl_re = Regex::new(
        r"(?m)^[\s]*@implementation\s+(\w+)"
    )?;

    // ObjC method: - (returnType)methodName:(paramType)param
    let method_re = Regex::new(
        r"(?m)^[\s]*[-+]\s*\([^)]+\)\s*(\w+)"
    )?;

    // ObjC property: @property (attributes) Type name;
    let property_re = Regex::new(
        r"(?m)^[\s]*@property\s*(?:\([^)]*\))?\s*\w+[\s*]*(\w+)\s*;"
    )?;

    // C typedef (common in ObjC headers)
    let typedef_re = Regex::new(
        r"(?m)^[\s]*typedef\s+(?:struct|enum|NS_ENUM|NS_OPTIONS)?\s*(?:\([^)]*\))?\s*\{?[^}]*\}?\s*(\w+)\s*;"
    )?;

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;

        // @interface
        if let Some(caps) = interface_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let mut parents = Vec::new();

            // Superclass
            if let Some(superclass) = caps.get(2) {
                parents.push((superclass.as_str().to_string(), "extends".to_string()));
            }

            // Protocols
            if let Some(protocols) = caps.get(3) {
                for proto in protocols.as_str().split(',') {
                    let proto = proto.trim();
                    if !proto.is_empty() {
                        parents.push((proto.to_string(), "implements".to_string()));
                    }
                }
            }

            // Check if it's a category (has parentheses after name)
            let is_category = line.contains(&format!("{}(", name)) ||
                              line.contains(&format!("{} (", name));

            if is_category {
                // ObjC category - treat like extension
                symbols.push(ParsedSymbol {
                    name: format!("{}+Category", name),
                    kind: SymbolKind::Object,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![(name, "extends".to_string())],
                });
            } else {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents,
                });
            }
        }

        // @protocol
        if let Some(caps) = protocol_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let mut parents = Vec::new();

            // Protocol inheritance
            if let Some(parent_protocols) = caps.get(2) {
                for proto in parent_protocols.as_str().split(',') {
                    let proto = proto.trim();
                    if !proto.is_empty() {
                        parents.push((proto.to_string(), "extends".to_string()));
                    }
                }
            }

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Interface, // Protocol ~ Interface
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // @implementation
        if let Some(caps) = impl_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

            // Skip if we already have @interface for this
            // Implementation is just a reference back to the class
            if !symbols.iter().any(|s| s.name == name && s.kind == SymbolKind::Class) {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Methods
        if let Some(caps) = method_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Function,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Properties
        if let Some(caps) = property_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Property,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Typedefs
        if let Some(caps) = typedef_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() && name != "NS_ENUM" && name != "NS_OPTIONS" {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::TypeAlias,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }
    }

    Ok(symbols)
}
