//! Language-specific parsers for symbol extraction
//!
//! This module contains parsers for different programming languages:
//! - Kotlin/Java (Android)
//! - Swift (iOS)
//! - Objective-C (iOS)
//! - Perl

pub mod kotlin;
pub mod objc;
pub mod perl;
pub mod swift;

use crate::db::SymbolKind;

/// A parsed symbol from source code
#[derive(Debug, Clone)]
pub struct ParsedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub signature: String,
    pub parents: Vec<(String, String)>, // (parent_name, inherit_kind)
}

/// A reference/usage of a symbol
#[derive(Debug, Clone)]
pub struct ParsedRef {
    pub name: String,
    pub line: usize,
    pub context: String,
}

use std::collections::HashSet;
use anyhow::Result;
use regex::Regex;

// Re-export parser functions
pub use kotlin::{parse_kotlin_symbols, parse_parents};
pub use objc::parse_objc_symbols;
pub use perl::parse_perl_symbols;
pub use swift::parse_swift_symbols;

/// Check if file extension is supported for indexing
pub fn is_supported_extension(ext: &str) -> bool {
    matches!(ext, "kt" | "java" | "swift" | "m" | "h" | "pm" | "pl" | "t")
}

/// Parse symbols and references from file content
pub fn parse_symbols_and_refs(
    content: &str,
    is_swift: bool,
    is_objc: bool,
    is_perl: bool,
) -> Result<(Vec<ParsedSymbol>, Vec<ParsedRef>)> {
    let symbols = if is_swift {
        parse_swift_symbols(content)?
    } else if is_objc {
        parse_objc_symbols(content)?
    } else if is_perl {
        parse_perl_symbols(content)?
    } else {
        parse_kotlin_symbols(content)?
    };
    let refs = extract_references(content, &symbols)?;
    Ok((symbols, refs))
}

/// Extract references/usages from file content
pub fn extract_references(content: &str, defined_symbols: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
    let mut refs = Vec::new();

    // Build set of locally defined symbol names (to skip them)
    let defined_names: HashSet<&str> = defined_symbols.iter().map(|s| s.name.as_str()).collect();

    // Regex for identifiers that might be references:
    // - CamelCase identifiers (types, classes) like PaymentRepository, String
    // - Function calls like getCards(, process(
    let identifier_re = Regex::new(r"\b([A-Z][a-zA-Z0-9]*)\b")?; // CamelCase types
    let func_call_re = Regex::new(r"\b([a-z][a-zA-Z0-9]*)\s*\(")?; // function calls

    // Keywords to skip
    let keywords: HashSet<&str> = [
        "if", "else", "when", "while", "for", "do", "try", "catch", "finally",
        "return", "break", "continue", "throw", "is", "in", "as", "true", "false",
        "null", "this", "super", "class", "interface", "object", "fun", "val", "var",
        "import", "package", "private", "public", "protected", "internal", "override",
        "abstract", "final", "open", "sealed", "data", "inner", "enum", "companion",
        "lateinit", "const", "suspend", "inline", "crossinline", "noinline", "reified",
        "annotation", "typealias", "get", "set", "init", "constructor", "by", "where",
        // Common standard library that would create too much noise
        "String", "Int", "Long", "Double", "Float", "Boolean", "Byte", "Short", "Char",
        "Unit", "Any", "Nothing", "List", "Map", "Set", "Array", "Pair", "Triple",
        "MutableList", "MutableMap", "MutableSet", "HashMap", "ArrayList", "HashSet",
        "Exception", "Error", "Throwable", "Result", "Sequence",
    ].into_iter().collect();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;
        let trimmed = line.trim();

        // Skip import/package declarations
        if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
            continue;
        }

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }

        // Extract CamelCase types (classes, interfaces, etc.)
        for caps in identifier_re.captures_iter(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() && !keywords.contains(name) && !defined_names.contains(name) {
                refs.push(ParsedRef {
                    name: name.to_string(),
                    line: line_num,
                    context: trimmed.to_string(),
                });
            }
        }

        // Extract function calls
        for caps in func_call_re.captures_iter(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() && !keywords.contains(name) && !defined_names.contains(name) {
                // Only add if name length > 2 to avoid noise
                if name.len() > 2 {
                    refs.push(ParsedRef {
                        name: name.to_string(),
                        line: line_num,
                        context: trimmed.to_string(),
                    });
                }
            }
        }
    }

    Ok(refs)
}
