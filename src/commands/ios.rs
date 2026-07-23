//! iOS-specific commands
//!
//! Commands for working with iOS/Swift codebases:
//! - storyboard_usages: Find storyboard usages of a class
//! - asset_usages: Find iOS asset usages
//! - swiftui: Find SwiftUI state properties
//! - async_funcs: Find Swift async functions
//! - publishers: Find Combine publishers
//! - main_actor: Find @MainActor usages

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use colored::Colorize;
use regex::Regex;

use super::{relative_path, search_files};
use crate::db;

/// Find storyboard usages of a class
pub fn cmd_storyboard_usages(root: &Path, class_name: &str, module: Option<&str>) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    let class_like = format!("%{}%", class_name);

    #[allow(clippy::type_complexity)]
    let results: Vec<(String, i64, String, Option<String>, Option<String>)> =
        if let Some(m) = module {
            let mod_like = format!("%{}%", m);
            let mut stmt = conn.prepare(
                r#"
            SELECT su.file_path, su.line, su.class_name, su.usage_type, su.storyboard_id
            FROM storyboard_usages su
            LEFT JOIN modules mod ON su.module_id = mod.id
            WHERE su.class_name LIKE ?1
            AND (mod.name LIKE ?2 OR mod.path LIKE ?2)
            ORDER BY su.file_path, su.line
            "#,
            )?;
            let rows: Vec<_> = stmt
                .query_map(rusqlite::params![class_like, mod_like], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        } else {
            let mut stmt = conn.prepare(
                r#"
            SELECT file_path, line, class_name, usage_type, storyboard_id
            FROM storyboard_usages
            WHERE class_name LIKE ?1
            ORDER BY file_path, line
            "#,
            )?;
            let rows: Vec<_> = stmt
                .query_map(rusqlite::params![class_like], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

    if results.is_empty() {
        println!(
            "{}",
            format!("No storyboard usages found for '{}'", class_name).yellow()
        );
    } else {
        println!(
            "{}",
            format!(
                "Storyboard usages for '{}' ({}):",
                class_name,
                results.len()
            )
            .bold()
        );
        for (path, line, cls, usage_type, sb_id) in &results {
            let type_str = usage_type.as_deref().unwrap_or("unknown");
            let id_str = sb_id
                .as_deref()
                .map(|s| format!(" (id: {})", s))
                .unwrap_or_default();
            println!(
                "  {}:{} {} [{}]{}",
                path.cyan(),
                line,
                cls,
                type_str,
                id_str
            );
        }
    }

    Ok(())
}

/// Find iOS asset usages
pub fn cmd_asset_usages(
    root: &Path,
    asset: &str,
    module: Option<&str>,
    asset_type: Option<&str>,
    unused: bool,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    if unused {
        // Find unused assets
        if module.is_none() {
            println!("{}", "Error: --unused requires --module".red());
            return Ok(());
        }

        let m = module.unwrap();
        let mod_like = format!("%{}%", m);

        let (query, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(t) = asset_type {
                (
                    r#"
                SELECT a.name, a.type, a.file_path
                FROM ios_assets a
                LEFT JOIN modules mod ON a.module_id = mod.id
                LEFT JOIN ios_asset_usages au ON a.id = au.asset_id
                WHERE (mod.name LIKE ?1 OR mod.path LIKE ?1)
                AND au.id IS NULL
                AND a.type = ?2
                ORDER BY a.type, a.name
                "#,
                    vec![
                        Box::new(mod_like) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(t.to_string()),
                    ],
                )
            } else {
                (
                    r#"
                SELECT a.name, a.type, a.file_path
                FROM ios_assets a
                LEFT JOIN modules mod ON a.module_id = mod.id
                LEFT JOIN ios_asset_usages au ON a.id = au.asset_id
                WHERE (mod.name LIKE ?1 OR mod.path LIKE ?1)
                AND au.id IS NULL
                ORDER BY a.type, a.name
                "#,
                    vec![Box::new(mod_like) as Box<dyn rusqlite::types::ToSql>],
                )
            };

        let mut stmt = conn.prepare(query)?;
        let results: Vec<(String, String, String)> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        if results.is_empty() {
            println!(
                "{}",
                format!("No unused assets found in module '{}'", m).green()
            );
        } else {
            println!(
                "{}",
                format!("Unused assets in '{}' ({}):", m, results.len()).bold()
            );
            for (name, atype, path) in &results {
                println!("  {} [{}]: {}", name.cyan(), atype, path.dimmed());
            }
        }
    } else if asset.is_empty() {
        // List all assets
        let results: Vec<(String, String, String)> = if let Some(t) = asset_type {
            let mut stmt = conn.prepare(
                "SELECT name, type, file_path FROM ios_assets WHERE type = ?1 ORDER BY type, name LIMIT 100",
            )?;
            stmt.query_map(rusqlite::params![t], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        } else {
            let mut stmt = conn.prepare(
                "SELECT name, type, file_path FROM ios_assets ORDER BY type, name LIMIT 100",
            )?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        println!("{}", format!("iOS assets ({}):", results.len()).bold());
        for (name, atype, path) in &results {
            println!("  {} [{}]: {}", name.cyan(), atype, path.dimmed());
        }
    } else {
        // Find usages of specific asset
        let asset_like = format!("%{}%", asset);
        let mut stmt = conn.prepare(
            r#"
            SELECT a.name, a.type, au.usage_file, au.usage_line
            FROM ios_assets a
            JOIN ios_asset_usages au ON a.id = au.asset_id
            WHERE a.name LIKE ?1
            ORDER BY au.usage_file, au.usage_line
            "#,
        )?;
        let results: Vec<(String, String, String, i64)> = stmt
            .query_map(rusqlite::params![asset_like], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        if results.is_empty() {
            println!(
                "{}",
                format!("No usages found for asset '{}'", asset).yellow()
            );
        } else {
            println!(
                "{}",
                format!("Usages of '{}' ({}):", asset, results.len()).bold()
            );
            for (name, atype, file, line) in &results {
                println!("  {} [{}]: {}:{}", name.cyan(), atype, file, line);
            }
        }
    }

    Ok(())
}

/// Find SwiftUI state properties using tree-sitter.
/// Finds any `@Wrapper var/let` property, not limited to a fixed list of wrappers.
pub fn cmd_swiftui(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    use crate::parsers::treesitter::swift::find_property_wrappers;

    // Use grep to find candidate files (fast), then tree-sitter for precise extraction
    let mut swift_files: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    search_files(
        root,
        r"@\w+.*\b(var|let)\b",
        &["swift"],
        |path, _line_num, _line| {
            swift_files.insert(path.to_path_buf());
        },
    )?;

    let mut results: Vec<(String, String, String, usize)> = vec![];

    for file_path in &swift_files {
        if results.len() >= limit {
            break;
        }
        if let Ok(content) = std::fs::read_to_string(file_path) {
            if let Ok(wrappers) = find_property_wrappers(&content) {
                for pw in wrappers {
                    if results.len() >= limit {
                        break;
                    }

                    if let Some(q) = query {
                        let q_lower = q.to_lowercase();
                        if !pw.name.to_lowercase().contains(&q_lower)
                            && !pw.wrapper.to_lowercase().contains(&q_lower)
                        {
                            continue;
                        }
                    }

                    let rel_path = relative_path(root, file_path);
                    results.push((pw.wrapper, pw.name, rel_path, pw.line));
                }
            }
        }
    }

    println!(
        "{}",
        format!("SwiftUI state properties ({}):", results.len()).bold()
    );

    // Group by type
    let mut by_type: HashMap<String, Vec<(String, String, usize)>> = HashMap::new();
    for (prop_type, prop_name, path, line) in results {
        by_type
            .entry(prop_type)
            .or_default()
            .push((prop_name, path, line));
    }

    for (prop_type, props) in &by_type {
        println!(
            "\n  {} ({}):",
            format!("@{}", prop_type).cyan(),
            props.len()
        );
        for (name, path, line) in props.iter().take(10) {
            println!("    {}: {}:{}", name, path, line);
        }
        if props.len() > 10 {
            println!("    ... and {} more", props.len() - 10);
        }
    }

    Ok(())
}

/// Find Swift async functions using tree-sitter.
/// Handles multi-line signatures natively.
pub fn cmd_async_funcs(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    use crate::parsers::treesitter::swift::find_async_funcs;

    let mut results: Vec<(String, String, usize)> = vec![];

    // Use grep to find candidate files (fast), then tree-sitter for precise extraction
    let mut swift_files: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    search_files(root, r"\basync\b", &["swift"], |path, _line_num, _line| {
        swift_files.insert(path.to_path_buf());
    })?;

    for file_path in &swift_files {
        if results.len() >= limit {
            break;
        }
        if let Ok(content) = std::fs::read_to_string(file_path) {
            if let Ok(funcs) = find_async_funcs(&content) {
                for f in funcs {
                    if results.len() >= limit {
                        break;
                    }

                    if let Some(q) = query {
                        if !f.name.to_lowercase().contains(&q.to_lowercase()) {
                            continue;
                        }
                    }

                    let rel_path = relative_path(root, file_path);
                    results.push((f.name, rel_path, f.line));
                }
            }
        }
    }

    println!("{}", format!("Async functions ({}):", results.len()).bold());

    for (func_name, path, line_num) in &results {
        println!("  {}: {}:{}", func_name.cyan(), path, line_num);
    }

    Ok(())
}

/// Find Combine publishers (PassthroughSubject, CurrentValueSubject, AnyPublisher)
pub fn cmd_publishers(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // Search for Combine publishers: PassthroughSubject, CurrentValueSubject, AnyPublisher, Published
    let pattern = r"(PassthroughSubject|CurrentValueSubject|AnyPublisher|@Published)\s*[<(]";

    let pub_regex = Regex::new(
        r"(PassthroughSubject|CurrentValueSubject|AnyPublisher)(?:\s*<[^>]+>)?\s*(?:\(\)|[,;=])|@Published\s+(?:private\s+)?var\s+(\w+)",
    )?;

    let mut results: Vec<(String, String, String, usize)> = vec![];

    search_files(root, pattern, &["swift"], |path, line_num, line| {
        if results.len() >= limit {
            return;
        }

        if let Some(caps) = pub_regex.captures(line) {
            let pub_type = caps.get(1).map(|m| m.as_str()).unwrap_or("@Published");
            let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");

            if let Some(q) = query {
                let q_lower = q.to_lowercase();
                if !pub_type.to_lowercase().contains(&q_lower)
                    && !name.to_lowercase().contains(&q_lower)
                    && !line.to_lowercase().contains(&q_lower)
                {
                    return;
                }
            }

            let rel_path = relative_path(root, path);
            let content: String = line.trim().chars().take(80).collect();
            results.push((pub_type.to_string(), content, rel_path, line_num));
        }
    })?;

    println!(
        "{}",
        format!("Combine publishers ({}):", results.len()).bold()
    );

    for (pub_type, content, path, line_num) in &results {
        println!("  {} {}:{}", pub_type.cyan(), path, line_num);
        println!("    {}", content.dimmed());
    }

    Ok(())
}

/// Find @MainActor usages
pub fn cmd_main_actor(root: &Path, query: Option<&str>, limit: usize) -> Result<()> {
    // Search for @MainActor
    let pattern = r"@MainActor";

    let mut results: Vec<(String, usize, String)> = vec![];

    search_files(root, pattern, &["swift"], |path, line_num, line| {
        if results.len() >= limit {
            return;
        }

        if let Some(q) = query {
            if !line.to_lowercase().contains(&q.to_lowercase()) {
                return;
            }
        }

        let rel_path = relative_path(root, path);
        let content: String = line.trim().chars().take(100).collect();
        results.push((rel_path, line_num, content));
    })?;

    println!(
        "{}",
        format!("@MainActor usages ({}):", results.len()).bold()
    );

    for (path, line_num, content) in &results {
        println!("  {}:{}", path.cyan(), line_num);
        println!("    {}", content);
    }

    Ok(())
}
