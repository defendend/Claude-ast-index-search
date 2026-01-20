#![allow(dead_code)]

use anyhow::Result;
use rayon::prelude::*;
use regex::Regex;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::db::{self, SymbolKind};

/// Project type detected by markers
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectType {
    Android,  // Kotlin/Java - build.gradle.kts, settings.gradle.kts
    IOS,      // Swift/ObjC - Package.swift, *.xcodeproj
    Mixed,    // Both markers present
    Unknown,
}

impl ProjectType {
    pub fn as_str(&self) -> &str {
        match self {
            ProjectType::Android => "Android (Kotlin/Java)",
            ProjectType::IOS => "iOS (Swift/ObjC)",
            ProjectType::Mixed => "Mixed (Android + iOS)",
            ProjectType::Unknown => "Unknown",
        }
    }
}

/// Detect project type by looking for marker files
pub fn detect_project_type(root: &Path) -> ProjectType {
    let has_gradle = root.join("settings.gradle.kts").exists()
        || root.join("settings.gradle").exists()
        || root.join("build.gradle.kts").exists()
        || root.join("build.gradle").exists();

    let has_swift = root.join("Package.swift").exists()
        || fs::read_dir(root)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().map(|ext| ext == "xcodeproj").unwrap_or(false))
            })
            .unwrap_or(false);

    // Also check subdirectories for Package.swift (SPM structure)
    let has_swift = has_swift || {
        fs::read_dir(root)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    let path = e.path();
                    path.is_dir() && path.join("Package.swift").exists()
                })
            })
            .unwrap_or(false)
    };

    match (has_gradle, has_swift) {
        (true, true) => ProjectType::Mixed,
        (true, false) => ProjectType::Android,
        (false, true) => ProjectType::IOS,
        (false, false) => ProjectType::Unknown,
    }
}

/// Index a single Kotlin/Java file
pub fn index_file(conn: &Connection, root: &Path, file_path: &Path) -> Result<()> {
    let metadata = fs::metadata(file_path)?;
    let mtime = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    let size = metadata.len() as i64;

    let rel_path = file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    // Insert or update file
    let file_id = db::upsert_file(conn, &rel_path, mtime, size)?;

    // Delete old symbols for this file (will be re-added)
    conn.execute(
        "DELETE FROM symbols WHERE file_id = ?1",
        rusqlite::params![file_id],
    )?;

    // Read and parse file
    let content = fs::read_to_string(file_path)?;
    parse_and_index(conn, file_id, &content)?;

    Ok(())
}

/// Parse file content and extract symbols
fn parse_and_index(conn: &Connection, file_id: i64, content: &str) -> Result<()> {
    // Regex patterns for Kotlin/Java constructs
    let class_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|abstract|open|final|sealed|data|value|inline|annotation|inner|enum)[\s]+)*(?:class|object)\s+(\w+)(?:\s*<[^>]*>)?(?:\s*\([^)]*\))?(?:\s*:\s*([^{]+))?"
    )?;

    let interface_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|sealed|fun)[\s]+)*interface\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    let fun_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|suspend|inline|operator|infix|tailrec|external|actual|expect)[\s]+)*fun\s+(?:<[^>]*>\s*)?(?:(\w+)\.)?(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\S+))?"
    )?;

    let property_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|const|lateinit|lazy)[\s]+)*(?:val|var)\s+(\w+)(?:\s*:\s*(\S+))?"
    )?;

    let typealias_re = Regex::new(r"(?m)^[\s]*typealias\s+(\w+)(?:\s*<[^>]*>)?\s*=\s*(.+)")?;

    let enum_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal)[\s]+)*enum\s+class\s+(\w+)"
    )?;

    // Index classes
    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1; // 1-indexed

        // Classes and objects
        if let Some(caps) = class_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let parents = caps.get(3).map(|m| m.as_str().trim());
            let is_object = line.contains("object ");

            let kind = if is_object {
                SymbolKind::Object
            } else {
                SymbolKind::Class
            };

            let signature = Some(line.trim());
            let symbol_id = db::insert_symbol(conn, file_id, name, kind, line_num, signature)?;

            // Index inheritance
            if let Some(parents_str) = parents {
                for parent in parse_parents(parents_str) {
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
                        db::insert_inheritance(conn, symbol_id, parent_name, inherit_kind)?;
                    }
                }
            }
        }

        // Interfaces
        if let Some(caps) = interface_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let parents = caps.get(3).map(|m| m.as_str().trim());
            let signature = Some(line.trim());

            let symbol_id =
                db::insert_symbol(conn, file_id, name, SymbolKind::Interface, line_num, signature)?;

            if let Some(parents_str) = parents {
                for parent in parse_parents(parents_str) {
                    let parent_name = parent.trim().split('<').next().unwrap_or("").trim();
                    if !parent_name.is_empty() {
                        db::insert_inheritance(conn, symbol_id, parent_name, "extends")?;
                    }
                }
            }
        }

        // Enums
        if let Some(caps) = enum_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let signature = Some(line.trim());
            db::insert_symbol(conn, file_id, name, SymbolKind::Enum, line_num, signature)?;
        }

        // Functions (top-level or member)
        if let Some(caps) = fun_re.captures(line) {
            let name = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let signature = Some(line.trim());
            db::insert_symbol(conn, file_id, name, SymbolKind::Function, line_num, signature)?;
        }

        // Properties (top-level or member)
        if let Some(caps) = property_re.captures(line) {
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if !name.is_empty() && name != "val" && name != "var" {
                let signature = Some(line.trim());
                db::insert_symbol(conn, file_id, name, SymbolKind::Property, line_num, signature)?;
            }
        }

        // Type aliases
        if let Some(caps) = typealias_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let signature = Some(line.trim());
            db::insert_symbol(conn, file_id, name, SymbolKind::TypeAlias, line_num, signature)?;
        }
    }

    Ok(())
}

/// Collect full class declaration spanning multiple lines
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
    let inheritance_re = Regex::new(r"\)\s*:\s*([^{]+)").ok();

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
        let simple_re = Regex::new(r"(?:class|object)\s+\w+(?:\s*<[^>]*>)?\s*:\s*([^{(]+)").ok();
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
fn parse_parents(parents_str: &str) -> Vec<&str> {
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

/// Parsed file data for parallel processing
struct ParsedFile {
    rel_path: String,
    mtime: i64,
    size: i64,
    symbols: Vec<ParsedSymbol>,
    refs: Vec<ParsedRef>,
}

struct ParsedSymbol {
    name: String,
    kind: SymbolKind,
    line: usize,
    signature: String,
    parents: Vec<(String, String)>, // (parent_name, inherit_kind)
}

/// A reference/usage of a symbol
struct ParsedRef {
    name: String,
    line: usize,
    context: String,
}

/// Parse a single file without DB access (thread-safe)
fn parse_file(root: &Path, file_path: &Path) -> Result<ParsedFile> {
    let metadata = fs::metadata(file_path)?;
    let mtime = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    let size = metadata.len() as i64;

    let rel_path = file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    let content = fs::read_to_string(file_path)?;

    // Detect file type by extension
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_swift = ext == "swift";
    let is_objc = ext == "m" || ext == "h";

    let (symbols, refs) = if is_objc {
        parse_symbols_and_refs(&content, false, true)?
    } else {
        parse_symbols_and_refs(&content, is_swift, false)?
    };

    Ok(ParsedFile {
        rel_path,
        mtime,
        size,
        symbols,
        refs,
    })
}

/// Parse symbols from file content
fn parse_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Simple regex for detecting class/interface start
    let class_start_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|abstract|open|final|sealed|data|value|inline|annotation|inner|enum)[\s]+)*(?:class|object)\s+(\w+)"
    )?;

    let interface_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|sealed|fun)[\s]+)*interface\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    let fun_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|suspend|inline|operator|infix|tailrec|external|actual|expect)[\s]+)*fun\s+(?:<[^>]*>\s*)?(?:(\w+)\.)?(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\S+))?"
    )?;

    let property_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|protected|internal|override|const|lateinit|lazy)[\s]+)*(?:val|var)\s+(\w+)(?:\s*:\s*(\S+))?"
    )?;

    let typealias_re = Regex::new(r"(?m)^[\s]*typealias\s+(\w+)(?:\s*<[^>]*>)?\s*=\s*(.+)")?;
    let enum_re = Regex::new(r"(?m)^[\s]*((?:public|private|protected|internal)[\s]+)*enum\s+class\s+(\w+)")?;

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
    }

    Ok(symbols)
}

/// Parse Swift symbols from file content
fn parse_swift_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
    let mut symbols = Vec::new();

    // Swift class: public/private/internal/final class ClassName: Parent, Protocol
    let class_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|open|final)\s+)*class\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    // Swift struct
    let struct_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate)\s+)*struct\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    // Swift enum
    let enum_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate)\s+)*enum\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    // Swift protocol
    let protocol_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate)\s+)*protocol\s+(\w+)(?:\s*:\s*([^{]+))?"
    )?;

    // Swift actor
    let actor_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|final)\s+)*actor\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    // Swift extension
    let extension_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|internal|fileprivate)\s+)?extension\s+(\w+)(?:\s*<[^>]*>)?(?:\s*:\s*([^{]+))?"
    )?;

    // Swift func (including async/throws)
    let func_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|open|final|override|static|class|mutating)\s+)*func\s+(\w+)\s*(?:<[^>]*>)?\s*\([^)]*\)"
    )?;

    // Swift init
    let init_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|internal|fileprivate|override|convenience|required)\s+)*init\s*(?:\?|!)?\s*\("
    )?;

    // Swift var/let properties
    let property_re = Regex::new(
        r"(?m)^[\s]*(@\w+\s+)*((?:public|private|internal|fileprivate|static|class|lazy|weak|unowned)\s+)*(var|let)\s+(\w+)\s*:"
    )?;

    // Swift typealias
    let typealias_re = Regex::new(
        r"(?m)^[\s]*((?:public|private|internal|fileprivate)\s+)?typealias\s+(\w+)(?:\s*<[^>]*>)?\s*="
    )?;

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
fn parse_swift_parents(parents_str: Option<&str>) -> Vec<(String, String)> {
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

/// Parse Objective-C symbols from file content
fn parse_objc_symbols(content: &str) -> Result<Vec<ParsedSymbol>> {
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

/// Parse symbols AND references from file content (combined for efficiency)
fn parse_symbols_and_refs(content: &str, is_swift: bool, is_objc: bool) -> Result<(Vec<ParsedSymbol>, Vec<ParsedRef>)> {
    let symbols = if is_swift {
        parse_swift_symbols(content)?
    } else if is_objc {
        parse_objc_symbols(content)?
    } else {
        parse_symbols(content)?
    };
    let refs = extract_references(content, &symbols)?;
    Ok((symbols, refs))
}

/// Extract references/usages from file content
fn extract_references(content: &str, defined_symbols: &[ParsedSymbol]) -> Result<Vec<ParsedRef>> {
    use std::collections::HashSet;

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

/// Check if file extension is supported for indexing
fn is_supported_extension(ext: &str) -> bool {
    matches!(ext, "kt" | "java" | "swift" | "m" | "h")
}

/// Index all Kotlin/Java/Swift/ObjC files in a directory (parallel parsing, batched DB writes)
pub fn index_directory(conn: &mut Connection, root: &Path, progress: bool) -> Result<usize> {
    use ignore::WalkBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Detect project type
    let project_type = detect_project_type(root);
    if progress {
        eprintln!("Detected project type: {}", project_type.as_str());
    }

    // Collect all file paths
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    let files: Vec<PathBuf> = walker
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(is_supported_extension)
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    let total_files = files.len();
    if progress {
        eprintln!("Found {} files to parse...", total_files);
    }

    // Parse files in parallel
    let parsed_count = Arc::new(AtomicUsize::new(0));
    let root_clone = root.to_path_buf();
    let parsed_count_clone = parsed_count.clone();

    let parsed_files: Vec<ParsedFile> = files
        .par_iter()
        .filter_map(|path| {
            let result = parse_file(&root_clone, path).ok();
            let c = parsed_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
            if progress && c % 2000 == 0 {
                eprintln!("Parsed {} / {} files...", c, total_files);
            }
            result
        })
        .collect();

    if progress {
        eprintln!("Parsed {} files, writing to database...", parsed_files.len());
    }

    // Write to database in a single transaction (much faster)
    let tx = conn.transaction()?;

    // Prepare statements once
    let mut file_stmt = tx.prepare_cached(
        "INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)"
    )?;
    let mut del_sym_stmt = tx.prepare_cached("DELETE FROM symbols WHERE file_id = ?1")?;
    let mut del_ref_stmt = tx.prepare_cached("DELETE FROM refs WHERE file_id = ?1")?;
    let mut sym_stmt = tx.prepare_cached(
        "INSERT INTO symbols (file_id, name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5)"
    )?;
    let mut inh_stmt = tx.prepare_cached(
        "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)"
    )?;
    let mut ref_stmt = tx.prepare_cached(
        "INSERT INTO refs (file_id, name, line, context) VALUES (?1, ?2, ?3, ?4)"
    )?;

    let mut count = 0;
    for pf in parsed_files {
        // Insert file
        file_stmt.execute(rusqlite::params![pf.rel_path, pf.mtime, pf.size])?;
        let file_id = tx.last_insert_rowid();

        // Delete old symbols and refs
        del_sym_stmt.execute(rusqlite::params![file_id])?;
        del_ref_stmt.execute(rusqlite::params![file_id])?;

        // Insert symbols
        for sym in pf.symbols {
            sym_stmt.execute(rusqlite::params![
                file_id,
                sym.name,
                sym.kind.as_str(),
                sym.line as i64,
                sym.signature
            ])?;
            let symbol_id = tx.last_insert_rowid();

            for (parent_name, inherit_kind) in sym.parents {
                inh_stmt.execute(rusqlite::params![symbol_id, parent_name, inherit_kind])?;
            }
        }

        // Insert references
        for r in pf.refs {
            ref_stmt.execute(rusqlite::params![file_id, r.name, r.line as i64, r.context])?;
        }

        count += 1;
        if progress && count % 5000 == 0 {
            eprintln!("Written {} / {} files to DB...", count, total_files);
        }
    }

    // Drop statements before commit
    drop(file_stmt);
    drop(del_sym_stmt);
    drop(del_ref_stmt);
    drop(sym_stmt);
    drop(inh_stmt);
    drop(ref_stmt);

    tx.commit()?;

    Ok(count)
}

/// Incremental update: only re-index changed/new files, delete removed files
pub fn update_directory_incremental(conn: &mut Connection, root: &Path, progress: bool) -> Result<(usize, usize, usize)> {
    use ignore::WalkBuilder;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // 1. Load existing files from DB with their mtime
    let mut existing_files: HashMap<String, (i64, i64)> = HashMap::new(); // path -> (file_id, mtime)
    {
        let mut stmt = conn.prepare("SELECT id, path, mtime FROM files")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })?;
        for row in rows {
            let (id, path, mtime) = row?;
            existing_files.insert(path, (id, mtime));
        }
    }

    if progress {
        eprintln!("Loaded {} files from index", existing_files.len());
    }

    // 2. Walk filesystem and collect files to update
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    let current_files: Vec<PathBuf> = walker
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(is_supported_extension)
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    // 3. Categorize files: new, changed, unchanged
    let mut files_to_parse: Vec<PathBuf> = Vec::new();
    let mut current_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    for file_path in &current_files {
        let rel_path = file_path
            .strip_prefix(root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();
        current_paths.insert(rel_path.clone());

        let file_mtime = fs::metadata(file_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        if let Some((_, db_mtime)) = existing_files.get(&rel_path) {
            if file_mtime > *db_mtime {
                // File changed
                files_to_parse.push(file_path.clone());
            }
            // else: unchanged, skip
        } else {
            // New file
            files_to_parse.push(file_path.clone());
        }
    }

    // 4. Find deleted files
    let deleted_paths: Vec<String> = existing_files
        .keys()
        .filter(|p| !current_paths.contains(*p))
        .cloned()
        .collect();

    if progress {
        eprintln!(
            "Found {} new/changed files, {} deleted files",
            files_to_parse.len(),
            deleted_paths.len()
        );
    }

    // 5. Delete removed files from DB
    if !deleted_paths.is_empty() {
        let tx = conn.transaction()?;
        {
            let mut del_file_stmt = tx.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            for path in &deleted_paths {
                del_file_stmt.execute(rusqlite::params![path])?;
            }
        }
        tx.commit()?;
    }

    // 6. Parse and update changed/new files
    let updated_count = if !files_to_parse.is_empty() {
        let total_files = files_to_parse.len();
        let parsed_count = Arc::new(AtomicUsize::new(0));
        let root_clone = root.to_path_buf();
        let parsed_count_clone = parsed_count.clone();

        let parsed_files: Vec<ParsedFile> = files_to_parse
            .par_iter()
            .filter_map(|path| {
                let result = parse_file(&root_clone, path).ok();
                let c = parsed_count_clone.fetch_add(1, Ordering::Relaxed) + 1;
                if progress && c % 500 == 0 {
                    eprintln!("Parsed {} / {} changed files...", c, total_files);
                }
                result
            })
            .collect();

        // Write to DB
        let tx = conn.transaction()?;
        {
            let mut file_stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)"
            )?;
            let mut del_sym_stmt = tx.prepare_cached("DELETE FROM symbols WHERE file_id = ?1")?;
            let mut del_ref_stmt = tx.prepare_cached("DELETE FROM refs WHERE file_id = ?1")?;
            let mut sym_stmt = tx.prepare_cached(
                "INSERT INTO symbols (file_id, name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5)"
            )?;
            let mut inh_stmt = tx.prepare_cached(
                "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)"
            )?;
            let mut ref_stmt = tx.prepare_cached(
                "INSERT INTO refs (file_id, name, line, context) VALUES (?1, ?2, ?3, ?4)"
            )?;

            for pf in &parsed_files {
                file_stmt.execute(rusqlite::params![pf.rel_path, pf.mtime, pf.size])?;
                let file_id = tx.last_insert_rowid();
                del_sym_stmt.execute(rusqlite::params![file_id])?;
                del_ref_stmt.execute(rusqlite::params![file_id])?;

                for sym in &pf.symbols {
                    sym_stmt.execute(rusqlite::params![
                        file_id,
                        sym.name,
                        sym.kind.as_str(),
                        sym.line as i64,
                        sym.signature
                    ])?;
                    let symbol_id = tx.last_insert_rowid();

                    for (parent_name, inherit_kind) in &sym.parents {
                        inh_stmt.execute(rusqlite::params![symbol_id, parent_name, inherit_kind])?;
                    }
                }

                // Insert refs
                for r in &pf.refs {
                    ref_stmt.execute(rusqlite::params![file_id, r.name, r.line as i64, r.context])?;
                }
            }
        }
        tx.commit()?;
        parsed_files.len()
    } else {
        0
    };

    Ok((updated_count, files_to_parse.len(), deleted_paths.len()))
}

/// Index modules from build.gradle files (Android) and Package.swift (iOS)
pub fn index_modules(conn: &Connection, root: &Path) -> Result<usize> {
    use ignore::WalkBuilder;

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    let mut count = 0;

    // Regex to extract SPM targets from Package.swift
    let spm_target_re = Regex::new(r#"\.(?:target|testTarget|binaryTarget)\s*\(\s*name:\s*["']([^"']+)["']"#)?;

    for entry in walker {
        let entry = entry?;
        let path = entry.path();

        if let Some(name) = path.file_name() {
            let name_str = name.to_string_lossy();

            // Android/Gradle modules
            if name_str == "build.gradle" || name_str == "build.gradle.kts" {
                if let Some(parent) = path.parent() {
                    let module_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    // Convert path to module name (e.g., features/payments/api -> features.payments.api)
                    let module_name = module_path.replace('/', ".");

                    if !module_name.is_empty() {
                        conn.execute(
                            "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                            rusqlite::params![module_name, module_path],
                        )?;
                        count += 1;
                    }
                }
            }

            // iOS/SPM modules (Package.swift)
            if name_str == "Package.swift" {
                if let Some(parent) = path.parent() {
                    let package_path = parent
                        .strip_prefix(root)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .to_string();

                    // Read Package.swift and extract targets
                    if let Ok(content) = fs::read_to_string(path) {
                        for caps in spm_target_re.captures_iter(&content) {
                            let target_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            if !target_name.is_empty() {
                                let module_name = if package_path.is_empty() {
                                    target_name.to_string()
                                } else {
                                    format!("{}.{}", package_path.replace('/', "."), target_name)
                                };
                                let module_path = if package_path.is_empty() {
                                    target_name.to_string()
                                } else {
                                    format!("{}/{}", package_path, target_name)
                                };

                                conn.execute(
                                    "INSERT OR IGNORE INTO modules (name, path) VALUES (?1, ?2)",
                                    rusqlite::params![module_name, module_path],
                                )?;
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(count)
}

/// Parse module dependencies from build.gradle files
pub fn index_module_dependencies(conn: &mut Connection, root: &Path, progress: bool) -> Result<usize> {
    use ignore::WalkBuilder;

    // Regex patterns for dependency declarations
    // Gradle projects DSL style: modules { api(projects.features.payments.api) }
    let projects_dep_re = Regex::new(r"(?m)^\s*(api|implementation|compileOnly|testImplementation)\s*\(\s*projects\.([a-zA-Z_][a-zA-Z0-9_.]*)\s*\)")?;

    // Standard Gradle style: implementation(project(":features:payments:api"))
    let gradle_project_re = Regex::new(r#"(?m)(api|implementation|compileOnly|testImplementation)\s*\(\s*project\s*\(\s*["']:([^"']+)["']\s*\)"#)?;

    // First, ensure all modules are indexed and get their IDs
    let module_ids: std::collections::HashMap<String, i64> = {
        let mut stmt = conn.prepare("SELECT id, name FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (name, id) = row?;
            map.insert(name, id);
        }
        map
    };

    if progress {
        eprintln!("Found {} modules in index", module_ids.len());
    }

    let mut dep_count = 0;
    let tx = conn.transaction()?;

    // Clear existing dependencies
    tx.execute("DELETE FROM module_deps", [])?;

    {
        let mut dep_stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO module_deps (module_id, dep_module_id, dep_kind) VALUES (?1, ?2, ?3)"
        )?;

        let walker = WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker {
            let entry = entry?;
            let path = entry.path();

            if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str == "build.gradle" || name_str == "build.gradle.kts" {
                    if let Some(parent) = path.parent() {
                        let module_path = parent
                            .strip_prefix(root)
                            .unwrap_or(parent)
                            .to_string_lossy()
                            .to_string();
                        let module_name = module_path.replace('/', ".");

                        if let Some(&module_id) = module_ids.get(&module_name) {
                            // Read build.gradle content
                            if let Ok(content) = fs::read_to_string(path) {
                                // Parse projects DSL style dependencies
                                for caps in projects_dep_re.captures_iter(&content) {
                                    let dep_kind = caps.get(1).map(|m| m.as_str()).unwrap_or("implementation");
                                    let dep_name = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                                    if let Some(&dep_id) = module_ids.get(dep_name) {
                                        dep_stmt.execute(rusqlite::params![module_id, dep_id, dep_kind])?;
                                        dep_count += 1;
                                    }
                                }

                                // Parse standard Gradle style dependencies
                                for caps in gradle_project_re.captures_iter(&content) {
                                    let dep_kind = caps.get(1).map(|m| m.as_str()).unwrap_or("implementation");
                                    let dep_path = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                                    // Convert :features:payments:api to features.payments.api
                                    let dep_name = dep_path.trim_start_matches(':').replace(':', ".");

                                    if let Some(&dep_id) = module_ids.get(&dep_name) {
                                        dep_stmt.execute(rusqlite::params![module_id, dep_id, dep_kind])?;
                                        dep_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    tx.commit()?;

    Ok(dep_count)
}

/// Get dependencies of a module
pub fn get_module_deps(conn: &Connection, module_name: &str) -> Result<Vec<(String, String, String)>> {
    // Returns (dep_module_name, dep_module_path, dep_kind)
    let mut stmt = conn.prepare(
        r#"
        SELECT m2.name, m2.path, md.dep_kind
        FROM module_deps md
        JOIN modules m1 ON md.module_id = m1.id
        JOIN modules m2 ON md.dep_module_id = m2.id
        WHERE m1.name = ?1 OR m1.path = ?1
        ORDER BY md.dep_kind, m2.name
        "#
    )?;

    let results = stmt
        .query_map(rusqlite::params![module_name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Get modules that depend on this module
pub fn get_module_dependents(conn: &Connection, module_name: &str) -> Result<Vec<(String, String, String)>> {
    // Returns (dependent_module_name, dependent_module_path, dep_kind)
    let mut stmt = conn.prepare(
        r#"
        SELECT m1.name, m1.path, md.dep_kind
        FROM module_deps md
        JOIN modules m1 ON md.module_id = m1.id
        JOIN modules m2 ON md.dep_module_id = m2.id
        WHERE m2.name = ?1 OR m2.path = ?1
        ORDER BY md.dep_kind, m1.name
        "#
    )?;

    let results = stmt
        .query_map(rusqlite::params![module_name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Parsed XML usage
#[derive(Debug)]
pub struct XmlUsage {
    pub file_path: String,
    pub line: usize,
    pub class_name: String,
    pub usage_type: String,
    pub element_id: Option<String>,
}

/// Index XML layouts for class usages
pub fn index_xml_usages(conn: &mut Connection, root: &Path, progress: bool) -> Result<usize> {
    use ignore::WalkBuilder;

    // Get module IDs map
    let module_ids: std::collections::HashMap<String, i64> = {
        let mut stmt = conn.prepare("SELECT id, path FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (path, id) = row?;
            map.insert(path, id);
        }
        map
    };

    // Regex for class names in XML
    // Full class name: <com.example.MyView ...>
    let full_class_re = Regex::new(r"<([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*\.[A-Z][a-zA-Z0-9_]*)")?;
    // view class="..." or fragment android:name="..."
    let class_attr_re = Regex::new(r#"(?:class|android:name)\s*=\s*["']([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*\.[A-Z][a-zA-Z0-9_]*)["']"#)?;
    // android:id="@+id/xxx"
    let id_re = Regex::new(r#"android:id\s*=\s*["']@\+?id/([^"']+)["']"#)?;

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    let xml_files: Vec<PathBuf> = walker
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.extension().map(|ext| ext == "xml").unwrap_or(false)
                && path.to_string_lossy().contains("/res/")
                && (path.to_string_lossy().contains("/layout")
                    || path.to_string_lossy().contains("/menu")
                    || path.to_string_lossy().contains("/navigation"))
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    if progress {
        eprintln!("Found {} XML layout files to index...", xml_files.len());
    }

    let tx = conn.transaction()?;

    // Clear existing XML usages
    tx.execute("DELETE FROM xml_usages", [])?;

    let mut count = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO xml_usages (module_id, file_path, line, class_name, usage_type, element_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )?;

        for xml_path in &xml_files {
            let rel_path = xml_path
                .strip_prefix(root)
                .unwrap_or(xml_path)
                .to_string_lossy()
                .to_string();

            // Find module for this file
            let module_id = module_ids.iter()
                .find(|(path, _)| rel_path.starts_with(*path))
                .map(|(_, id)| *id);

            if let Ok(content) = fs::read_to_string(xml_path) {
                for (line_num, line) in content.lines().enumerate() {
                    let line_num = line_num + 1;

                    // Extract element_id if present on this line
                    let element_id = id_re.captures(line).map(|c| c.get(1).unwrap().as_str().to_string());

                    // Full class name tags
                    for caps in full_class_re.captures_iter(line) {
                        let class_name = caps.get(1).unwrap().as_str();
                        stmt.execute(rusqlite::params![
                            module_id,
                            rel_path,
                            line_num as i64,
                            class_name,
                            "view_tag",
                            element_id
                        ])?;
                        count += 1;
                    }

                    // class="..." or android:name="..." attributes
                    for caps in class_attr_re.captures_iter(line) {
                        let class_name = caps.get(1).unwrap().as_str();
                        let usage_type = if line.contains("<fragment") || line.contains("android:name") {
                            "fragment"
                        } else {
                            "view_class_attr"
                        };
                        stmt.execute(rusqlite::params![
                            module_id,
                            rel_path,
                            line_num as i64,
                            class_name,
                            usage_type,
                            element_id
                        ])?;
                        count += 1;
                    }
                }
            }
        }
    }

    tx.commit()?;

    Ok(count)
}

/// Resource type
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceType {
    Drawable,
    String,
    Color,
    Dimen,
    Style,
    Layout,
    Id,
    Mipmap,
    Other(String),
}

impl ResourceType {
    pub fn as_str(&self) -> &str {
        match self {
            ResourceType::Drawable => "drawable",
            ResourceType::String => "string",
            ResourceType::Color => "color",
            ResourceType::Dimen => "dimen",
            ResourceType::Style => "style",
            ResourceType::Layout => "layout",
            ResourceType::Id => "id",
            ResourceType::Mipmap => "mipmap",
            ResourceType::Other(s) => s,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "drawable" => ResourceType::Drawable,
            "string" => ResourceType::String,
            "color" => ResourceType::Color,
            "dimen" => ResourceType::Dimen,
            "style" => ResourceType::Style,
            "layout" => ResourceType::Layout,
            "id" => ResourceType::Id,
            "mipmap" => ResourceType::Mipmap,
            other => ResourceType::Other(other.to_string()),
        }
    }
}

/// Index Android resources (drawable, string, color, etc.)
pub fn index_resources(conn: &mut Connection, root: &Path, progress: bool) -> Result<(usize, usize)> {
    use ignore::WalkBuilder;

    // Get module IDs map
    let module_ids: std::collections::HashMap<String, i64> = {
        let mut stmt = conn.prepare("SELECT id, path FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (path, id) = row?;
            map.insert(path, id);
        }
        map
    };

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    // Collect resource files
    let res_files: Vec<PathBuf> = walker
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.to_string_lossy().contains("/res/")
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    if progress {
        eprintln!("Found {} resource files to analyze...", res_files.len());
    }

    let tx = conn.transaction()?;

    // Clear existing resources
    tx.execute("DELETE FROM resource_usages", [])?;
    tx.execute("DELETE FROM resources", [])?;

    let mut resource_count = 0;
    let mut usage_count = 0;

    // Regex for resource references
    let r_ref_re = Regex::new(r"R\.(drawable|string|color|dimen|style|layout|id|mipmap)\.([a-zA-Z_][a-zA-Z0-9_]*)")?;
    let xml_ref_re = Regex::new(r#"@(drawable|string|color|dimen|style|layout|id|mipmap)/([a-zA-Z_][a-zA-Z0-9_]*)"#)?;

    // Resource definitions regex for values/*.xml
    let string_def_re = Regex::new(r#"<string\s+name="([^"]+)""#)?;
    let color_def_re = Regex::new(r#"<color\s+name="([^"]+)""#)?;
    let dimen_def_re = Regex::new(r#"<dimen\s+name="([^"]+)""#)?;
    let style_def_re = Regex::new(r#"<style\s+name="([^"]+)""#)?;

    {
        let mut res_stmt = tx.prepare_cached(
            "INSERT INTO resources (module_id, type, name, file_path, line) VALUES (?1, ?2, ?3, ?4, ?5)"
        )?;

        // First pass: index resource definitions
        for res_path in &res_files {
            let rel_path = res_path
                .strip_prefix(root)
                .unwrap_or(res_path)
                .to_string_lossy()
                .to_string();

            let module_id = module_ids.iter()
                .find(|(path, _)| rel_path.starts_with(*path))
                .map(|(_, id)| *id);

            // Drawable files
            if rel_path.contains("/drawable") || rel_path.contains("/mipmap") {
                if let Some(name) = res_path.file_stem().and_then(|n| n.to_str()) {
                    let res_type = if rel_path.contains("/mipmap") { "mipmap" } else { "drawable" };
                    res_stmt.execute(rusqlite::params![module_id, res_type, name, rel_path, 1])?;
                    resource_count += 1;
                }
            }

            // Layout files
            if rel_path.contains("/layout") && rel_path.ends_with(".xml") {
                if let Some(name) = res_path.file_stem().and_then(|n| n.to_str()) {
                    res_stmt.execute(rusqlite::params![module_id, "layout", name, rel_path, 1])?;
                    resource_count += 1;
                }
            }

            // Values files (strings, colors, dimens, styles)
            if rel_path.contains("/values") && rel_path.ends_with(".xml") {
                if let Ok(content) = fs::read_to_string(res_path) {
                    for (line_num, line) in content.lines().enumerate() {
                        let line_num = line_num + 1;

                        if let Some(caps) = string_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![module_id, "string", name, rel_path, line_num as i64])?;
                            resource_count += 1;
                        }
                        if let Some(caps) = color_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![module_id, "color", name, rel_path, line_num as i64])?;
                            resource_count += 1;
                        }
                        if let Some(caps) = dimen_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![module_id, "dimen", name, rel_path, line_num as i64])?;
                            resource_count += 1;
                        }
                        if let Some(caps) = style_def_re.captures(line) {
                            let name = caps.get(1).unwrap().as_str();
                            res_stmt.execute(rusqlite::params![module_id, "style", name, rel_path, line_num as i64])?;
                            resource_count += 1;
                        }
                    }
                }
            }
        }
    }

    // Build resource ID map
    let resource_ids: std::collections::HashMap<(String, String), i64> = {
        let mut stmt = tx.prepare("SELECT id, type, name FROM resources")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, res_type, name) = row?;
            map.insert((res_type, name), id);
        }
        map
    };

    // Second pass: index resource usages
    {
        let mut usage_stmt = tx.prepare_cached(
            "INSERT INTO resource_usages (resource_id, usage_file, usage_line, usage_type) VALUES (?1, ?2, ?3, ?4)"
        )?;

        let walker = WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .build();

        let code_files: Vec<PathBuf> = walker
            .filter_map(|e| e.ok())
            .filter(|e| {
                let ext = e.path().extension().and_then(|s| s.to_str());
                matches!(ext, Some("kt") | Some("java") | Some("xml"))
            })
            .map(|e| e.path().to_path_buf())
            .collect();

        for file_path in &code_files {
            let rel_path = file_path
                .strip_prefix(root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            if let Ok(content) = fs::read_to_string(file_path) {
                let is_xml = rel_path.ends_with(".xml");

                for (line_num, line) in content.lines().enumerate() {
                    let line_num = line_num + 1;

                    // R.type.name references (Kotlin/Java)
                    if !is_xml {
                        for caps in r_ref_re.captures_iter(line) {
                            let res_type = caps.get(1).unwrap().as_str();
                            let res_name = caps.get(2).unwrap().as_str();

                            if let Some(&resource_id) = resource_ids.get(&(res_type.to_string(), res_name.to_string())) {
                                usage_stmt.execute(rusqlite::params![resource_id, rel_path, line_num as i64, "code"])?;
                                usage_count += 1;
                            }
                        }
                    }

                    // @type/name references (XML)
                    for caps in xml_ref_re.captures_iter(line) {
                        let res_type = caps.get(1).unwrap().as_str();
                        let res_name = caps.get(2).unwrap().as_str();

                        if let Some(&resource_id) = resource_ids.get(&(res_type.to_string(), res_name.to_string())) {
                            usage_stmt.execute(rusqlite::params![resource_id, rel_path, line_num as i64, "xml"])?;
                            usage_count += 1;
                        }
                    }
                }
            }
        }
    }

    tx.commit()?;

    Ok((resource_count, usage_count))
}

/// Build transitive dependencies cache
pub fn build_transitive_deps(conn: &mut Connection, progress: bool) -> Result<usize> {
    // Get all direct dependencies
    let direct_deps: Vec<(i64, i64, String)> = {
        let mut stmt = conn.prepare("SELECT module_id, dep_module_id, dep_kind FROM module_deps")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    // Get module names
    let module_names: std::collections::HashMap<i64, String> = {
        let mut stmt = conn.prepare("SELECT id, name FROM modules")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.insert(id, name);
        }
        map
    };

    // Build adjacency list (only api dependencies create transitive access)
    let mut api_deps: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for (module_id, dep_id, dep_kind) in &direct_deps {
        if dep_kind == "api" {
            api_deps.entry(*module_id).or_default().push(*dep_id);
        }
    }

    let tx = conn.transaction()?;

    // Clear existing
    tx.execute("DELETE FROM transitive_deps", [])?;

    let mut count = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO transitive_deps (module_id, dependency_id, depth, path) VALUES (?1, ?2, ?3, ?4)"
        )?;

        // For each module, BFS to find all transitive dependencies
        for (module_id, dep_id, _) in &direct_deps {
            // Direct dependency
            let path = format!("{} -> {}",
                module_names.get(module_id).unwrap_or(&"?".to_string()),
                module_names.get(dep_id).unwrap_or(&"?".to_string())
            );
            stmt.execute(rusqlite::params![module_id, dep_id, 1, path])?;
            count += 1;

            // BFS for transitive (only through api deps)
            let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
            visited.insert(*dep_id);
            let mut queue: std::collections::VecDeque<(i64, usize, String)> = std::collections::VecDeque::new();

            // Add api dependencies of dep_id
            if let Some(next_deps) = api_deps.get(dep_id) {
                for &next_dep in next_deps {
                    let next_path = format!("{} -> {} -> {}",
                        module_names.get(module_id).unwrap_or(&"?".to_string()),
                        module_names.get(dep_id).unwrap_or(&"?".to_string()),
                        module_names.get(&next_dep).unwrap_or(&"?".to_string())
                    );
                    queue.push_back((next_dep, 2, next_path));
                }
            }

            while let Some((trans_dep, depth, path)) = queue.pop_front() {
                if visited.contains(&trans_dep) || depth > 5 {
                    continue;
                }
                visited.insert(trans_dep);

                stmt.execute(rusqlite::params![module_id, trans_dep, depth as i64, path])?;
                count += 1;

                // Continue BFS
                if let Some(next_deps) = api_deps.get(&trans_dep) {
                    for &next_dep in next_deps {
                        if !visited.contains(&next_dep) {
                            let next_path = format!("{} -> {}",
                                path,
                                module_names.get(&next_dep).unwrap_or(&"?".to_string())
                            );
                            queue.push_back((next_dep, depth + 1, next_path));
                        }
                    }
                }
            }
        }
    }

    tx.commit()?;

    if progress {
        eprintln!("Built {} transitive dependency entries", count);
    }

    Ok(count)
}
