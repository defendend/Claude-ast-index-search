//! Perl symbol parser
//!
//! Parses Perl source files (.pm, .pl, .t) to extract:
//! - Package declarations
//! - Subroutine definitions
//! - Constants (use constant)
//! - Our variables
//! - Inheritance (use base, use parent, @ISA)

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::db::SymbolKind;
use super::ParsedSymbol;

/// Parse Perl source code and extract symbols
pub fn parse_perl_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Regex patterns for Perl constructs
    // Package declaration: package Name;
    static PACKAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)\s*;").unwrap());
    let package_re = &*PACKAGE_RE;

    // Subroutine definition: sub name { } or sub name($proto) { }
    static SUB_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)\s*[\{(]?").unwrap());

    let sub_re = &*SUB_RE;

    // Constant definition: use constant NAME => value;
    static CONSTANT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*use\s+constant\s+([A-Z_][A-Z0-9_]*)\s*=>").unwrap());

    let constant_re = &*CONSTANT_RE;

    // Our variable declaration: our $VAR, our @ARRAY, our %HASH
    static OUR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*our\s+([\$@%][A-Za-z_][A-Za-z0-9_]*)").unwrap());

    let our_re = &*OUR_RE;

    // Inheritance patterns
    // use base qw/Parent1 Parent2/; or use base 'Parent';
    static USE_BASE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"use\s+(?:base|parent)\s+(?:qw[/(]([^)/\\]+)[)/\\]|['"]([^'"]+)['"])"#).unwrap());

    let use_base_re = &*USE_BASE_RE;
    // our @ISA = qw(Parent1 Parent2);
    static ISA_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"our\s+@ISA\s*=\s*(?:qw[/(]([^)/\\]+)[)/\\]|\(([^)]+)\))"#).unwrap());

    let isa_re = &*ISA_RE;

    // Track current package for context
    let mut current_package: Option<(String, i64)> = None; // (name, symbol_id placeholder)
    let mut pending_parents: Vec<(String, String)> = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;

        // Package declaration
        if let Some(caps) = package_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                // Apply any pending parents to this package
                let parents = std::mem::take(&mut pending_parents);
                symbols.push(ParsedSymbol {
                    name: name.clone(),
                    kind: SymbolKind::Package,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents,
                });
                current_package = Some((name, symbols.len() as i64 - 1));
            }
            continue;
        }

        // Subroutine definition
        if let Some(caps) = sub_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
            continue;
        }

        // Constant definition
        if let Some(caps) = constant_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Constant,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
            continue;
        }

        // Our variable (but not @ISA which is handled separately)
        if let Some(caps) = our_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            // Skip @ISA as it's for inheritance, not a real variable to index
            if !name.is_empty() && name != "@ISA" {
                symbols.push(ParsedSymbol {
                    name,
                    kind: SymbolKind::Property,
                    line: line_num,
                    signature: line.trim().to_string(),
                    parents: vec![],
                });
            }
        }

        // Inheritance: use base/parent
        if let Some(caps) = use_base_re.captures(line) {
            let parents_str = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str());
            if let Some(ps) = parents_str {
                for parent in ps.split_whitespace() {
                    let parent_name = parent.trim();
                    if !parent_name.is_empty() {
                        let parent_entry = (parent_name.to_string(), "extends".to_string());
                        // If we have a current package, add to its parents
                        if let Some((_, idx)) = &current_package {
                            let idx = *idx as usize;
                            if idx < symbols.len() {
                                symbols[idx].parents.push(parent_entry);
                            }
                        } else {
                            // No package yet, save for later
                            pending_parents.push(parent_entry);
                        }
                    }
                }
            }
        }

        // Inheritance: @ISA
        if let Some(caps) = isa_re.captures(line) {
            let parents_str = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str());
            if let Some(ps) = parents_str {
                for parent in ps.split(|c: char| c.is_whitespace() || c == ',') {
                    let parent_name = parent.trim().trim_matches(|c| c == '\'' || c == '"');
                    if !parent_name.is_empty() {
                        let parent_entry = (parent_name.to_string(), "extends".to_string());
                        if let Some((_, idx)) = &current_package {
                            let idx = *idx as usize;
                            if idx < symbols.len() {
                                symbols[idx].parents.push(parent_entry);
                            }
                        } else {
                            pending_parents.push(parent_entry);
                        }
                    }
                }
            }
        }
    }

    Ok(symbols)
}
