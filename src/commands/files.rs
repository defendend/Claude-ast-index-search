//! File operation commands
//!
//! Commands for working with files:
//! - file: Find files by pattern
//! - outline: Show file symbols outline
//! - imports: Show file imports
//! - api: Show module public API
//! - changed: See [`super::changed`]

use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use rusqlite::OptionalExtension;

use crate::db::SymbolKind;

use super::{relative_path, search_files};
use crate::db;

/// Outline helper: parse file with tree-sitter and print symbols, skipping specified kinds.
/// Returns true if any symbols were printed.
fn outline_via_treesitter(
    content: &str,
    file_type: crate::parsers::FileType,
    skip_kinds: &[SymbolKind],
) -> Result<bool> {
    let (symbols, _refs) = crate::parsers::parse_file_symbols(content, file_type)?;
    let mut found = false;
    for sym in &symbols {
        if skip_kinds.contains(&sym.kind) {
            continue;
        }
        println!(
            "  {} {} [{}]",
            format!(":{}", sym.line).dimmed(),
            sym.name.cyan(),
            sym.kind.as_str()
        );
        found = true;
    }
    Ok(found)
}

/// Find files by pattern
pub fn cmd_file(root: &Path, pattern: &str, exact: bool, limit: usize, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    let search_pattern = if exact {
        pattern.to_string()
    } else {
        pattern.to_string()
    };
    let files = db::find_files_with_roots(&conn, &search_pattern, limit)?;
    let resolver = super::PathResolver::try_from_conn(root, &conn)?;
    let files: Vec<String> = files
        .into_iter()
        .filter(|f| resolver.matches_filter(f.root_path.as_deref()))
        .map(|file| resolver.resolve_with_root(&file.path, file.root_path.as_deref()))
        .collect();

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&files)?);
        return Ok(());
    }

    println!("{}", format!("Files matching '{}':", pattern).bold());

    for path in &files {
        println!("  {}", path);
    }

    if files.is_empty() {
        println!("  No files found.");
    }

    Ok(())
}

/// Show file symbols outline
pub fn cmd_outline(root: &Path, file: &str) -> Result<()> {
    // Find the file
    let file_path = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        root.join(file)
    };

    if !file_path.exists() {
        println!("{}", format!("File not found: {}", file).red());
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)?;

    // Detect file type
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    println!("{}", format!("Outline of {}:", file).bold());

    let mut found = false;

    if ext == "pm" || ext == "pl" || ext == "t" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Perl,
            &[SymbolKind::Import],
        )?;
    } else if ext == "py" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Python,
            &[SymbolKind::Import, SymbolKind::Property],
        )?;
    } else if ext == "go" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Go,
            &[SymbolKind::Import],
        )?;
    } else if ext == "cpp" || ext == "cc" || ext == "c" || ext == "hpp" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Cpp,
            &[SymbolKind::Import],
        )?;
    } else if ext == "h" {
        // .h may be C++, C or ObjC — sniff content
        let ft = if crate::parsers::detect_h_file_objc(&content) {
            crate::parsers::FileType::ObjC
        } else {
            crate::parsers::FileType::Cpp
        };
        found = outline_via_treesitter(&content, ft, &[SymbolKind::Import])?;
    } else if ext == "kt" || ext == "kts" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Kotlin,
            &[SymbolKind::Import],
        )?;
    } else if ext == "dart" {
        // Dart — delegate to tree-sitter parser for correct results
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Dart,
            &[SymbolKind::Import, SymbolKind::Property],
        )?;
    } else if ext == "java" {
        // Java — delegate to tree-sitter
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Java,
            &[SymbolKind::Import, SymbolKind::Annotation],
        )?;
    } else if ext == "ts" || ext == "tsx" || ext == "mts" || ext == "js" || ext == "jsx" {
        // TypeScript/JavaScript — delegate to tree-sitter
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::TypeScript,
            &[SymbolKind::Import],
        )?;
    } else if ext == "vue" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Vue,
            &[SymbolKind::Import],
        )?;
    } else if ext == "svelte" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Svelte,
            &[SymbolKind::Import],
        )?;
    } else if ext == "swift" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Swift,
            &[SymbolKind::Import],
        )?;
    } else if ext == "rb" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Ruby,
            &[SymbolKind::Import],
        )?;
    } else if ext == "rs" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Rust,
            &[SymbolKind::Import],
        )?;
    } else if ext == "scala" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::Scala,
            &[SymbolKind::Import],
        )?;
    } else if ext == "cs" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::CSharp,
            &[SymbolKind::Import],
        )?;
    } else if ext == "proto" {
        found = outline_via_treesitter(&content, crate::parsers::FileType::Proto, &[])?;
    } else if ext == "m" {
        let ft = crate::parsers::FileType::detect_m_file_type(&content);
        found = outline_via_treesitter(&content, ft, &[SymbolKind::Import])?;
    } else if ext == "mm" {
        found = outline_via_treesitter(
            &content,
            crate::parsers::FileType::ObjC,
            &[SymbolKind::Import],
        )?;
    } else if ext == "bsl" || ext == "os" {
        found = outline_via_treesitter(&content, crate::parsers::FileType::Bsl, &[])?;
    } else if let Some(ft) = crate::parsers::FileType::from_extension(ext) {
        // Any remaining language known to FileType::from_extension
        found = outline_via_treesitter(&content, ft, &[SymbolKind::Import])?;
    } else {
        println!("  Unsupported file type: .{}", ext);
    }

    if !found {
        println!("  No symbols found.");
    }

    Ok(())
}

/// Show file imports
pub fn cmd_imports(root: &Path, file: &str) -> Result<()> {
    let file_path = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        root.join(file)
    };

    if !file_path.exists() {
        println!("{}", format!("File not found: {}", file).red());
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)?;

    // Detect file type by extension
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_perl = ext == "pm" || ext == "pl" || ext == "t";
    let is_python = ext == "py";
    let is_go = ext == "go";
    let is_cpp = ext == "cpp" || ext == "cc" || ext == "c" || ext == "hpp" || ext == "h";

    println!("{}", format!("Imports in {}:", file).bold());

    let mut imports: Vec<String> = vec![];

    if is_perl {
        // Perl: use Module; or require Module;
        let use_re = Regex::new(r"^\s*(use|require)\s+([A-Za-z][A-Za-z0-9_:]*)")?;
        for line in content.lines() {
            if let Some(caps) = use_re.captures(line) {
                let keyword = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let module = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                // Skip pragmas
                if module != "strict"
                    && module != "warnings"
                    && module != "utf8"
                    && module != "constant"
                    && module != "base"
                    && module != "parent"
                    && !module.starts_with("v5")
                    && !module.starts_with("5.")
                {
                    imports.push(format!("{} {}", keyword, module));
                }
            }
        }
    } else if is_python {
        // Python: import module or from module import something
        let import_re = Regex::new(r"^import\s+([A-Za-z_][A-Za-z0-9_\.]*)")?;
        let from_re = Regex::new(r"^from\s+([A-Za-z_][A-Za-z0-9_\.]*)\s+import\s+(.+)")?;
        for line in content.lines() {
            if let Some(caps) = from_re.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let what = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                imports.push(format!("from {} import {}", module, what));
            } else if let Some(caps) = import_re.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                imports.push(format!("import {}", module));
            }
        }
    } else if is_go {
        // Go: import "module" or import ( "module1" "module2" )
        let single_import_re = Regex::new(r#"^import\s+"([^"]+)""#)?;
        let import_block_start = Regex::new(r"^import\s*\(")?;
        let import_line_re = Regex::new(r#"^\s*(?:[a-zA-Z_][a-zA-Z0-9_]*\s+)?"([^"]+)""#)?;

        let mut in_import_block = false;
        for line in content.lines() {
            if in_import_block {
                if line.trim() == ")" {
                    in_import_block = false;
                } else if let Some(caps) = import_line_re.captures(line) {
                    let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    imports.push(module.to_string());
                }
            } else if import_block_start.is_match(line) {
                in_import_block = true;
            } else if let Some(caps) = single_import_re.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                imports.push(module.to_string());
            }
        }
    } else if is_cpp {
        // C++: #include <header> or #include "header"
        let include_re = Regex::new(r#"^\s*#include\s*[<"]([^>"]+)[>"]"#)?;
        for line in content.lines() {
            if let Some(caps) = include_re.captures(line) {
                let header = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                imports.push(header.to_string());
            }
        }
    } else {
        // Kotlin/Java/Swift: import statement
        let import_re = Regex::new(r"(?m)^import\s+(.+)")?;
        for line in content.lines() {
            if let Some(caps) = import_re.captures(line) {
                imports.push(caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string());
            }
        }
    }

    if imports.is_empty() {
        println!("  No imports found.");
    } else {
        for imp in &imports {
            println!("  {}", imp);
        }
        println!("\n  Total: {} imports", imports.len());
    }

    Ok(())
}

/// Show module public API
pub fn cmd_api(root: &Path, module_path: &str, limit: usize) -> Result<()> {
    let mut module_dir = root.join(module_path);

    // If path not found, try converting dots to slashes (module name → path)
    if !module_dir.exists() && module_path.contains('.') {
        let converted = module_path.replace('.', "/");
        let alt = root.join(&converted);
        if alt.exists() {
            module_dir = alt;
        }
    }

    // Also try looking up module path from DB
    if !module_dir.exists() {
        if let Some(_cache_lease) = crate::db::acquire_project_lease_if_initialized(root)? {
            let conn = crate::db::open_db_leased(root)?;
            let db_path: Option<String> = conn
                .query_row(
                    "SELECT path FROM modules WHERE name = ?1",
                    rusqlite::params![module_path],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(p) = db_path {
                let alt = root.join(&p);
                if alt.exists() {
                    module_dir = alt;
                }
            }
        }
    }

    if !module_dir.exists() {
        println!("{}", format!("Module not found: {}", module_path).red());
        return Ok(());
    }

    // Find public classes, interfaces, functions in the module
    let pattern = r"(public\s+)?(class|interface|object|fun)\s+\w+";

    let mut items: Vec<(String, usize, String)> = vec![];

    search_files(
        &module_dir,
        pattern,
        &["kt", "java"],
        |path, line_num, line| {
            if items.len() >= limit {
                return;
            }

            // Skip private/internal
            if line.contains("private ") || line.contains("internal ") {
                return;
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(100).collect();
            items.push((rel_path, line_num, content));
        },
    )?;

    println!(
        "{}",
        format!("Public API of '{}' ({}):", module_path, items.len()).bold()
    );

    for (path, line_num, content) in &items {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    if items.is_empty() {
        println!("  No public API found.");
    }

    Ok(())
}

/// Compatibility entry point for the legacy changed command API.
#[deprecated(note = "use commands::changed::cmd_changed for structured changed-file output")]
pub fn cmd_changed(root: &Path, base: &str) -> Result<()> {
    super::changed::cmd_changed(root, Some(base), 30_000, false, "text")
}

/// Compatibility entry point for callers that detect the repository VCS.
#[deprecated(note = "the changed command now detects its VCS internally")]
pub fn detect_vcs(root: &Path) -> &'static str {
    super::changed::detect_vcs_compat(root)
}

/// Compatibility entry point for callers that detect the default Git base.
#[deprecated(note = "omit --base to let commands::changed::cmd_changed detect the Git base")]
pub fn detect_git_default_branch(root: &Path) -> &'static str {
    super::changed::detect_git_default_branch_compat(root)
}
