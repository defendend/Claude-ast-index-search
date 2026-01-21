//! Index-based search commands
//!
//! Commands for searching through the code index:
//! - search: Full-text search across files and symbols
//! - symbol: Find symbol by name
//! - class: Find class by name
//! - implementations: Find implementations of interface/class
//! - hierarchy: Show class hierarchy
//! - usages: Find symbol usages (indexed or grep-based)

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use rusqlite::{params, Connection};

use crate::db;
use super::{search_files, relative_path};

/// Full-text search across files, symbols, and file contents
pub fn cmd_search(root: &Path, query: &str, limit: usize) -> Result<()> {
    let total_start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // 1. Search in file paths (index)
    let files_start = Instant::now();
    let files = db::find_files(&conn, query, limit)?;
    let files_time = files_start.elapsed();

    // 2. Search in symbols using FTS (index)
    let symbols_start = Instant::now();
    let fts_query = format!("{}*", query); // Prefix search
    let symbols = db::search_symbols(&conn, &fts_query, limit)?;
    let symbols_time = symbols_start.elapsed();

    // 3. Search in file contents (grep)
    let content_start = Instant::now();
    let pattern = regex::escape(query);
    let mut content_matches: Vec<(String, usize, String)> = vec![];

    super::search_files_limited(root, &pattern, &["kt", "java", "swift", "m", "h", "py", "go", "rs", "cpp", "c", "proto"], limit, |path, line_num, line| {
        let rel_path = super::relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        content_matches.push((rel_path, line_num, content));
    })?;
    let content_time = content_start.elapsed();

    // Output results
    println!("{}", format!("Search results for '{}':", query).bold());

    if !files.is_empty() {
        println!("\n{}", "Files (by path):".cyan());
        for path in files.iter().take(limit) {
            println!("  {}", path);
        }
        if files.len() > limit {
            println!("  ... and {} more", files.len() - limit);
        }
    }

    if !symbols.is_empty() {
        println!("\n{}", "Symbols:".cyan());
        for s in symbols.iter().take(limit) {
            println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
        }
    }

    if !content_matches.is_empty() {
        println!("\n{}", "Content matches:".cyan());
        for (path, line_num, content) in content_matches.iter().take(limit) {
            println!("  {}:{}", path.cyan(), line_num);
            println!("    {}", content.dimmed());
        }
        if content_matches.len() > limit {
            println!("  ... and {} more", content_matches.len() - limit);
        }
    }

    if files.is_empty() && symbols.is_empty() && content_matches.is_empty() {
        println!("  No results found.");
    }

    // Timing breakdown
    eprintln!("\n{}", format!(
        "Time: {:?} (files: {:?}, symbols: {:?}, content: {:?})",
        total_start.elapsed(), files_time, symbols_time, content_time
    ).dimmed());
    Ok(())
}

/// Find symbol by name
pub fn cmd_symbol(root: &Path, name: &str, kind: Option<&str>, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let symbols = db::find_symbols_by_name(&conn, name, kind, limit)?;

    let kind_str = kind.map(|k| format!(" ({})", k)).unwrap_or_default();
    println!(
        "{}",
        format!("Symbols matching '{}'{}:", name, kind_str).bold()
    );

    for s in &symbols {
        println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
        if let Some(sig) = &s.signature {
            let truncated: String = sig.chars().take(70).collect();
            println!("    {}", truncated.dimmed());
        }
    }

    if symbols.is_empty() {
        println!("  No symbols found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find class by name (classes, interfaces, objects, enums)
pub fn cmd_class(root: &Path, name: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Single query for all class-like symbols
    let results = db::find_class_like(&conn, name, limit)?;

    println!("{}", format!("Classes matching '{}':", name).bold());

    for s in &results {
        println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
    }

    if results.is_empty() {
        println!("  No classes found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find implementations of interface/class
pub fn cmd_implementations(root: &Path, parent: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let impls = db::find_implementations(&conn, parent, limit)?;

    println!(
        "{}",
        format!("Implementations of '{}':", parent).bold()
    );

    for s in &impls {
        println!("  {} [{}]: {}:{}", s.name.cyan(), s.kind, s.path, s.line);
    }

    if impls.is_empty() {
        println!("  No implementations found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show class hierarchy (parents and children)
pub fn cmd_hierarchy(root: &Path, name: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Find the class/interface/package
    let classes = db::find_symbols_by_name(&conn, name, Some("class"), 1)?;
    let interfaces = db::find_symbols_by_name(&conn, name, Some("interface"), 1)?;
    let packages = db::find_symbols_by_name(&conn, name, Some("package"), 1)?;
    let protocols = db::find_symbols_by_name(&conn, name, Some("protocol"), 1)?;

    let target = classes.first().or(interfaces.first()).or(packages.first()).or(protocols.first());

    if target.is_none() {
        println!("{}", format!("Class '{}' not found.", name).red());
        return Ok(());
    }

    println!("{}", format!("Hierarchy for '{}':", name).bold());

    // Find parents
    let mut stmt = conn.prepare(
        "SELECT i.parent_name, i.kind FROM inheritance i JOIN symbols s ON i.child_id = s.id WHERE s.name = ?1",
    )?;
    let parents: Vec<(String, String)> = stmt
        .query_map([name], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    if !parents.is_empty() {
        println!("\n  {}", "Parents:".cyan());
        for (parent, kind) in &parents {
            println!("    {} ({})", parent, kind);
        }
    }

    // Find children
    let children = db::find_implementations(&conn, name, 20)?;
    if !children.is_empty() {
        println!("\n  {}", "Children:".cyan());
        for c in &children {
            println!("    {} [{}]", c.name, c.kind);
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find symbol usages (indexed or grep-based)
pub fn cmd_usages(root: &Path, symbol: &str, limit: usize) -> Result<()> {
    let start = Instant::now();

    // Try to use index first
    let db_path = db::get_db_path(root)?;
    if db_path.exists() {
        let conn = Connection::open(&db_path)?;

        // Check if refs table has data
        let refs_count: i64 = conn.query_row("SELECT COUNT(*) FROM refs WHERE name = ?1 LIMIT 1", params![symbol], |row| row.get(0)).unwrap_or(0);

        if refs_count > 0 {
            // Use indexed references
            let refs = db::find_references(&conn, symbol, limit)?;

            println!("{}", format!("Usages of '{}' ({}):", symbol, refs.len()).bold());

            for r in &refs {
                println!("  {}:{}", r.path.cyan(), r.line);
                if let Some(ctx) = &r.context {
                    let truncated: String = ctx.chars().take(80).collect();
                    println!("    {}", truncated);
                }
            }

            if refs.is_empty() {
                println!("  No usages found in index.");
            }

            eprintln!("\n{}", format!("Time: {:?} (indexed)", start.elapsed()).dimmed());
            return Ok(());
        }
    }

    // Fallback to grep-based search
    let pattern = format!(r"\b{}\b", regex::escape(symbol));
    let def_pattern = Regex::new(&format!(
        r"(class|interface|object|fun|val|var|typealias)\s+{}\b",
        regex::escape(symbol)
    ))?;

    let mut usages: Vec<(String, usize, String)> = vec![];

    search_files(root, &pattern, &["kt", "java"], |path, line_num, line| {
        if usages.len() >= limit { return; }

        // Skip definitions
        if def_pattern.is_match(line) { return; }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(80).collect();
        usages.push((rel_path, line_num, content));
    })?;

    println!("{}", format!("Usages of '{}' ({}):", symbol, usages.len()).bold());

    for (path, line_num, content) in &usages {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    if usages.is_empty() {
        println!("  No usages found.");
    }

    eprintln!("\n{}", format!("Time: {:?} (grep)", start.elapsed()).dimmed());
    Ok(())
}
