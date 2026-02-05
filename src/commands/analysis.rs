//! Code analysis commands
//!
//! - unused-symbols: Find potentially unused public symbols

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use rusqlite::params;

use crate::db;

/// Find potentially unused symbols in a module or project
pub fn cmd_unused_symbols(
    root: &Path,
    module: Option<&str>,
    export_only: bool,
    limit: usize,
    format: &str,
) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Build query based on filters
    let (sql, filter_param) = if let Some(mod_path) = module {
        (
            r#"
            SELECT s.name, s.kind, s.line, s.signature, f.path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE f.path LIKE ?1
              AND s.kind IN ('class', 'interface', 'function', 'object', 'enum', 'protocol', 'struct')
            ORDER BY f.path, s.line
            "#,
            Some(format!("{}%", mod_path)),
        )
    } else if export_only {
        (
            r#"
            SELECT s.name, s.kind, s.line, s.signature, f.path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.kind IN ('class', 'interface', 'function', 'object', 'enum', 'protocol', 'struct')
              AND s.name GLOB '[A-Z]*'
            ORDER BY f.path, s.line
            "#,
            None,
        )
    } else {
        (
            r#"
            SELECT s.name, s.kind, s.line, s.signature, f.path
            FROM symbols s
            JOIN files f ON s.file_id = f.id
            WHERE s.kind IN ('class', 'interface', 'function', 'object', 'enum', 'protocol', 'struct')
            ORDER BY f.path, s.line
            "#,
            None,
        )
    };

    let mut stmt = conn.prepare(sql)?;
    let symbols: Vec<db::SearchResult> = if let Some(ref pattern) = filter_param {
        stmt.query_map(params![pattern], |row| {
            Ok(db::SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], |row| {
            Ok(db::SearchResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                line: row.get(2)?,
                signature: row.get(3)?,
                path: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
    };

    // Check each symbol for references
    let mut unused: Vec<&db::SearchResult> = Vec::new();

    for sym in &symbols {
        // Check refs table
        let ref_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM refs WHERE name = ?1 LIMIT 1",
                params![sym.name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if ref_count > 0 {
            continue;
        }

        // Check xml_usages
        let xml_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM xml_usages WHERE class_name = ?1 LIMIT 1",
                params![sym.name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if xml_count > 0 {
            continue;
        }

        // Check storyboard_usages
        let sb_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM storyboard_usages WHERE class_name = ?1 LIMIT 1",
                params![sym.name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if sb_count > 0 {
            continue;
        }

        unused.push(sym);
        if unused.len() >= limit {
            break;
        }
    }

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&unused)?);
        return Ok(());
    }

    let scope = module.unwrap_or("project");
    println!(
        "{}",
        format!(
            "Potentially unused symbols in '{}' ({}/{} checked):",
            scope,
            unused.len(),
            symbols.len()
        )
        .bold()
    );

    for s in &unused {
        println!("  {} [{}]: {}:{}", s.name.yellow(), s.kind, s.path, s.line);
    }

    if unused.is_empty() {
        println!("  No unused symbols found.");
    }

    eprintln!(
        "\n{}",
        format!("Time: {:?}", start.elapsed()).dimmed()
    );
    Ok(())
}
