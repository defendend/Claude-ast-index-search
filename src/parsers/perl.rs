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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package() {
        let content = "package My::Module;\n";
        let symbols = parse_perl_symbols(content).unwrap();
        let pkg = symbols.iter().find(|s| s.name == "My::Module").unwrap();
        assert_eq!(pkg.kind, SymbolKind::Package);
    }

    #[test]
    fn test_parse_subroutine() {
        let content = "sub process_data {\n    my ($self) = @_;\n}\n";
        let symbols = parse_perl_symbols(content).unwrap();
        let f = symbols.iter().find(|s| s.name == "process_data").unwrap();
        assert_eq!(f.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_constant() {
        let content = "use constant MAX_RETRIES => 3;\n";
        let symbols = parse_perl_symbols(content).unwrap();
        let c = symbols.iter().find(|s| s.name == "MAX_RETRIES").unwrap();
        assert_eq!(c.kind, SymbolKind::Constant);
    }

    #[test]
    fn test_parse_our_variable() {
        let content = "our $VERSION = '1.0';\nour @EXPORT = qw(foo bar);\nour %CONFIG;\n";
        let symbols = parse_perl_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "$VERSION" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "@EXPORT" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "%CONFIG" && s.kind == SymbolKind::Property));
    }

    #[test]
    fn test_skip_isa() {
        let content = "our @ISA = qw(Parent);\n";
        let symbols = parse_perl_symbols(content).unwrap();
        assert!(!symbols.iter().any(|s| s.name == "@ISA"), "should skip @ISA variable");
    }

    #[test]
    fn test_parse_use_base_inheritance() {
        let content = "package Child;\nuse base qw/Parent1 Parent2/;\n";
        let symbols = parse_perl_symbols(content).unwrap();
        let pkg = symbols.iter().find(|s| s.name == "Child").unwrap();
        assert!(pkg.parents.iter().any(|(p, k)| p == "Parent1" && k == "extends"));
        assert!(pkg.parents.iter().any(|(p, k)| p == "Parent2" && k == "extends"));
    }

    #[test]
    fn test_parse_use_parent_inheritance() {
        let content = "package MyModule;\nuse parent 'Base::Class';\n";
        let symbols = parse_perl_symbols(content).unwrap();
        let pkg = symbols.iter().find(|s| s.name == "MyModule").unwrap();
        assert!(pkg.parents.iter().any(|(p, _)| p == "Base::Class"));
    }

    #[test]
    fn test_parse_isa_inheritance() {
        let content = "package Derived;\nour @ISA = qw(Base1 Base2);\n";
        let symbols = parse_perl_symbols(content).unwrap();
        let pkg = symbols.iter().find(|s| s.name == "Derived").unwrap();
        assert!(pkg.parents.iter().any(|(p, _)| p == "Base1"));
        assert!(pkg.parents.iter().any(|(p, _)| p == "Base2"));
    }

    #[test]
    fn test_full_perl_module() {
        let content = r#"package My::Service;
use base qw/My::Base/;

use constant TIMEOUT => 30;

our $VERSION = '2.0';

sub new {
    my ($class, %args) = @_;
    return bless \%args, $class;
}

sub process {
    my ($self, $data) = @_;
}

1;
"#;
        let symbols = parse_perl_symbols(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "My::Service" && s.kind == SymbolKind::Package));
        assert!(symbols.iter().any(|s| s.name == "TIMEOUT" && s.kind == SymbolKind::Constant));
        assert!(symbols.iter().any(|s| s.name == "$VERSION" && s.kind == SymbolKind::Property));
        assert!(symbols.iter().any(|s| s.name == "new" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "process" && s.kind == SymbolKind::Function));

        let pkg = symbols.iter().find(|s| s.name == "My::Service").unwrap();
        assert!(pkg.parents.iter().any(|(p, _)| p == "My::Base"));
    }
}
