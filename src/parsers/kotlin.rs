//! Kotlin/Java symbol parser
//!
//! Parses Kotlin and Java source files (.kt, .java) to extract:
//! - Classes, Objects
//! - Interfaces
//! - Enums
//! - Functions
//! - Properties (val/var)
//! - Type aliases

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Kotlin/Java source code and extract symbols
pub fn parse_kotlin_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Simple regex for detecting class/interface start
    static CLASS_START_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|abstract|open|final|sealed|data|value|inline|annotation|inner|enum)[\s]+)*(?:class|object)\s+(\w+)"

    ).unwrap());

    let class_start_re = &*CLASS_START_RE;

    static INTERFACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|sealed|fun)[\s]+)*interface\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"


    ).unwrap());


    let interface_re = &*INTERFACE_RE;

    static FUN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|suspend|inline|operator|infix|tailrec|external|actual|expect)[\s]+)*fun\s+(?:<[^>]*>\s*)?(?:(\w+)\.)?(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\S+))?"


    ).unwrap());


    let fun_re = &*FUN_RE;

    static PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|const|lateinit|lazy)[\s]+)*(?:val|var)\s+(\w+)(?:\s*:\s*(\S+))?"


    ).unwrap());


    let property_re = &*PROPERTY_RE;

    static TYPEALIAS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[\s]*typealias\s+(\w+)(?:\s*<[^>]*>)?\s*=\s*(.+)").unwrap());


    let typealias_re = &*TYPEALIAS_RE;
    static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[\s]*((?:public|private|protected|internal)[\s]+)*enum\s+class\s+(\w+)").unwrap());

    let enum_re = &*ENUM_RE;

    // Java static fields: public static final Type NAME = value;
    static JAVA_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"(?m)^[\s]*((?:public|private|protected)[\s]+)?(?:static[\s]+)?(?:final[\s]+)?(\w+(?:<[^>]+>)?)\s+([A-Z][A-Z0-9_]*)\s*="

    ).unwrap());

    let java_field_re = &*JAVA_FIELD_RE;

    let lines: Vec<&str> = content.lines().collect();

    for (line_num, line) in lines.iter().enumerate() {
        let line_num = line_num + 1;

        // Classes and objects - handle multiline declarations
        if let Some(caps) = class_start_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            let is_object = line.contains("object ");
            let kind = if is_object { SymbolKind::Object } else { SymbolKind::Class };

            // Collect full declaration (may span multiple lines)
            let full_decl = collect_class_declaration(&lines, line_num - 1);
            let parents = extract_parents_from_declaration(&full_decl);

            symbols.push(ParsedSymbol {
                name,
                kind,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Interfaces
        if let Some(caps) = interface_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            let parents_str = caps.get(3).map(|m| m.as_str().trim());

            let mut parents = Vec::new();
            if let Some(ps) = parents_str {
                for parent in parse_parents(ps) {
                    let parent_name = parent.trim().split('<').next().unwrap_or("").trim();
                    if !parent_name.is_empty() {
                        parents.push((parent_name.to_string(), "extends".to_string()));
                    }
                }
            }

            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Interface,
                line: line_num,
                signature: line.trim().to_string(),
                parents,
            });
        }

        // Enums
        if let Some(caps) = enum_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::Enum,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Functions
        if let Some(caps) = fun_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
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
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() && name != "val" && name != "var" {
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
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            symbols.push(ParsedSymbol {
                name,
                kind: SymbolKind::TypeAlias,
                line: line_num,
                signature: line.trim().to_string(),
                parents: vec![],
            });
        }

        // Java static fields (e.g., public static final String FOO = "bar";)
        if let Some(caps) = java_field_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
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
    }

    Ok(symbols)
}

/// Collect a full class declaration that may span multiple lines
fn collect_class_declaration(lines: &[&str], start_idx: usize) -> String {
    let mut result = String::new();
    let mut paren_depth = 0;
    let mut found_opening_brace = false;

    for i in start_idx..lines.len().min(start_idx + 20) { // Max 20 lines
        let line = lines[i];
        result.push_str(line);
        result.push(' ');

        for c in line.chars() {
            match c {
                '(' => paren_depth += 1,
                ')' => paren_depth -= 1,
                '{' => {
                    found_opening_brace = true;
                    break;
                }
                _ => {}
            }
        }

        // Stop when we found the opening brace (end of declaration)
        if found_opening_brace {
            break;
        }

        // Also stop if parentheses are balanced and we see ':'
        if paren_depth == 0 && line.contains(':') && i > start_idx {
            // Check if next line starts the body
            if i + 1 < lines.len() && lines[i + 1].trim().starts_with('{') {
                break;
            }
        }
    }

    result
}

/// Extract parent classes/interfaces from a full class declaration
fn extract_parents_from_declaration(decl: &str) -> Vec<(String, String)> {
    let mut parents = Vec::new();

    // Find the inheritance clause after ')' followed by ':'
    // Pattern: ClassName(...) : Parent1, Parent2 {
    static INHERITANCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\)\s*:\s*([^{]+)").unwrap());

    let inheritance_re = Some(&*INHERITANCE_RE);

    if let Some(re) = inheritance_re {
        if let Some(caps) = re.captures(decl) {
            if let Some(parents_str) = caps.get(1) {
                for parent in parse_parents(parents_str.as_str()) {
                    let inherit_kind = if parent.contains("()") {
                        "extends"
                    } else {
                        "implements"
                    };
                    let parent_name = parent
                        .trim()
                        .trim_end_matches("()")
                        .split('<')
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !parent_name.is_empty() {
                        parents.push((parent_name.to_string(), inherit_kind.to_string()));
                    }
                }
            }
        }
    }

    // Also check for simple inheritance (class Name : Parent)
    if parents.is_empty() {
        static SIMPLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:class|object)\s+\w+(?:\s*<[^>]*>)?\s*:\s*([^{(]+)").unwrap());

        let simple_re = Some(&*SIMPLE_RE);
        if let Some(re) = simple_re {
            if let Some(caps) = re.captures(decl) {
                if let Some(parents_str) = caps.get(1) {
                    for parent in parse_parents(parents_str.as_str()) {
                        let parent_name = parent
                            .trim()
                            .trim_end_matches("()")
                            .split('<')
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !parent_name.is_empty() {
                            parents.push((parent_name.to_string(), "implements".to_string()));
                        }
                    }
                }
            }
        }
    }

    parents
}

/// Parse parent classes/interfaces from inheritance clause
pub fn parse_parents(parents_str: &str) -> Vec<&str> {
    // Split by comma, handling generics
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in parents_str.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                let parent = parents_str[start..i].trim();
                if !parent.is_empty() {
                    result.push(parent);
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    // Add last parent
    let last = parents_str[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }

    result
}
