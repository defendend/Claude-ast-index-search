//! Module-related commands
//!
//! Commands for working with project modules:
//! - module: Find modules by pattern
//! - deps: Show module dependencies
//! - dependents: Show modules that depend on a module
//! - module-route: Show dependency path(s) between two modules
//! - unused_deps: Find unused dependencies

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use regex::Regex;
use rusqlite::{params, Connection};
use walkdir::WalkDir;

use crate::db;
use crate::indexer;

/// Find modules by pattern
pub fn cmd_module(root: &Path, pattern: &str, limit: usize) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    let mut stmt = conn.prepare("SELECT name, path FROM modules WHERE name LIKE ?1 LIMIT ?2")?;
    let pattern = format!("%{}%", pattern);
    let modules: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![pattern, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<_, _>>()?;

    println!("{}", format!("Modules matching '{}':", pattern).bold());

    for (name, path) in &modules {
        println!("  {}: {}", name.cyan(), path);
    }

    if modules.is_empty() {
        println!("  No modules found.");
    }

    Ok(())
}

/// Show module dependencies
pub fn cmd_deps(root: &Path, module: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    // Check if module deps are indexed
    if db::count_module_deps(&conn)? == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'ast-index rebuild' to index them.".yellow()
        );
        return Ok(());
    }

    let deps = indexer::get_module_deps(&conn, module)?;

    println!(
        "{}",
        format!("Dependencies of '{}' ({}):", module, deps.len()).bold()
    );

    // Group by kind
    let api_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k == "api").collect();
    let impl_deps: Vec<_> = deps
        .iter()
        .filter(|(_, _, k)| k == "implementation")
        .collect();
    let other_deps: Vec<_> = deps
        .iter()
        .filter(|(_, _, k)| k != "api" && k != "implementation")
        .collect();

    if !api_deps.is_empty() {
        println!("  {}:", "api".cyan());
        for (name, path, _) in &api_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !impl_deps.is_empty() {
        println!("  {}:", "implementation".cyan());
        for (name, path, _) in &impl_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !other_deps.is_empty() {
        println!("  {}:", "other".cyan());
        for (name, path, kind) in &other_deps {
            println!("    {} ({}) [{}]", name, path, kind);
        }
    }

    if deps.is_empty() {
        println!("  No dependencies found.");
    }

    Ok(())
}

/// Show modules that depend on a module
pub fn cmd_dependents(root: &Path, module: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    // Check if module deps are indexed
    if db::count_module_deps(&conn)? == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'ast-index rebuild' to index them.".yellow()
        );
        return Ok(());
    }

    let dependents = indexer::get_module_dependents(&conn, module)?;

    println!(
        "{}",
        format!("Modules depending on '{}' ({}):", module, dependents.len()).bold()
    );

    // Group by kind
    let api_deps: Vec<_> = dependents.iter().filter(|(_, _, k)| k == "api").collect();
    let impl_deps: Vec<_> = dependents
        .iter()
        .filter(|(_, _, k)| k == "implementation")
        .collect();
    let other_deps: Vec<_> = dependents
        .iter()
        .filter(|(_, _, k)| k != "api" && k != "implementation")
        .collect();

    if !api_deps.is_empty() {
        println!("  {} ({}):", "via api".cyan(), api_deps.len());
        for (name, path, _) in &api_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !impl_deps.is_empty() {
        println!("  {} ({}):", "via implementation".cyan(), impl_deps.len());
        for (name, path, _) in &impl_deps {
            println!("    {} ({})", name, path);
        }
    }

    if !other_deps.is_empty() {
        println!("  {} ({}):", "via other".cyan(), other_deps.len());
        for (name, path, kind) in &other_deps {
            println!("    {} ({}) [{}]", name, path, kind);
        }
    }

    if dependents.is_empty() {
        println!("  No dependents found.");
    }

    Ok(())
}

/// Find unused dependencies in a module
pub fn cmd_unused_deps(
    root: &Path,
    module: &str,
    verbose: bool,
    check_transitive: bool,
    check_xml: bool,
    check_resources: bool,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    // Check if module deps are indexed
    if db::count_module_deps(&conn)? == 0 {
        println!(
            "{}",
            "Module dependencies not indexed. Run 'ast-index rebuild' first.".yellow()
        );
        return Ok(());
    }

    // Get module id and path
    let module_info: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, path FROM modules WHERE name = ?1",
            params![module],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    let (module_id, module_path) = match module_info {
        Some((id, p)) => (id, p),
        None => {
            println!(
                "{}",
                format!("Module '{}' not found in index.", module).red()
            );
            return Ok(());
        }
    };

    // Get all dependencies
    let deps = indexer::get_module_deps(&conn, module)?;

    if deps.is_empty() {
        println!(
            "{}",
            format!("Module '{}' has no dependencies.", module).yellow()
        );
        return Ok(());
    }

    println!(
        "{}",
        format!("Analyzing {} dependencies of '{}'...", deps.len(), module).bold()
    );
    if check_transitive || check_xml || check_resources {
        let checks: Vec<&str> = [
            if check_transitive {
                Some("transitive")
            } else {
                None
            },
            if check_xml { Some("XML") } else { None },
            if check_resources {
                Some("resources")
            } else {
                None
            },
        ]
        .into_iter()
        .flatten()
        .collect();
        println!("  Checking: direct imports + {}\n", checks.join(", "));
    } else {
        println!("  Checking: direct imports only (strict mode)\n");
    }

    // Results tracking
    #[derive(Default)]
    struct DepUsage {
        direct_count: usize,
        direct_symbols: Vec<String>,
        transitive_count: usize,
        transitive_via: Vec<(String, Vec<String>)>, // (intermediate_module, symbols)
        xml_count: usize,
        xml_usages: Vec<(String, i64)>, // (class_name, line)
        resource_count: usize,
        resource_usages: Vec<(String, String)>, // (resource_name, usage_type)
    }

    let mut dep_usages: HashMap<String, DepUsage> = HashMap::new();
    let mut unused: Vec<(String, String, String)> = vec![];
    let mut exported: Vec<(String, String, String)> = vec![]; // api deps not directly used
    let mut used_direct: Vec<(String, String, String, usize)> = vec![];
    let mut used_transitive: Vec<(String, String, String, usize)> = vec![];
    let mut used_xml: Vec<(String, String, String, usize)> = vec![];
    let mut used_resources: Vec<(String, String, String, usize)> = vec![];

    for (dep_name, dep_path, dep_kind) in &deps {
        let mut usage = DepUsage::default();

        // 1. Check direct usage via index (refs table)
        let dep_symbols = get_module_public_symbols(&conn, root, dep_path)?;
        let (direct_count, direct_names) =
            count_symbols_used_in_module(&conn, &dep_symbols, &module_path)?;
        usage.direct_count = direct_count;
        usage.direct_symbols = direct_names;

        // 2. Check transitive usage (via api dependency chain in transitive_deps table)
        if check_transitive && usage.direct_count == 0 {
            let trans_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM transitive_deps td
                 JOIN modules m ON td.dependency_id = m.id
                 WHERE td.module_id = ?1 AND m.name = ?2 AND td.depth > 1",
                    params![module_id, dep_name],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            if trans_count > 0 {
                let path: String = conn
                    .query_row(
                        "SELECT td.path FROM transitive_deps td
                     JOIN modules m ON td.dependency_id = m.id
                     WHERE td.module_id = ?1 AND m.name = ?2 AND td.depth > 1
                     ORDER BY td.depth LIMIT 1",
                        params![module_id, dep_name],
                        |row| row.get(0),
                    )
                    .unwrap_or_default();

                usage.transitive_count = 1;
                let parts: Vec<&str> = path.split(" -> ").collect();
                if parts.len() >= 2 {
                    usage
                        .transitive_via
                        .push((parts[1].to_string(), vec!["(api chain)".to_string()]));
                }
            }
        }

        // 3. Check XML usages
        if check_xml && usage.direct_count == 0 && usage.transitive_count == 0 {
            // Get classes from the dependency module
            let mut class_stmt = conn.prepare(
                "SELECT DISTINCT s.name FROM symbols s
                 JOIN files f ON s.file_id = f.id
                 WHERE f.path LIKE ?1 AND s.kind IN ('class', 'object')
                 LIMIT 50",
            )?;
            let dep_pattern = format!("{}%", dep_path);
            let classes: Vec<String> = class_stmt
                .query_map(params![dep_pattern], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            // Check if any class is used in XML layouts of the target module
            for class_name in &classes {
                let mut xml_stmt = conn.prepare(
                    "SELECT x.file_path, x.line FROM xml_usages x
                     JOIN modules m ON x.module_id = m.id
                     WHERE m.id = ?1 AND x.class_name LIKE ?2",
                )?;
                let class_pattern = format!("%{}", class_name);
                let xml_results: Vec<(String, i64)> = xml_stmt
                    .query_map(params![module_id, class_pattern], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();

                for (_file_path, line) in xml_results {
                    usage.xml_count += 1;
                    if usage.xml_usages.len() < 3 {
                        usage.xml_usages.push((class_name.clone(), line));
                    }
                }
            }
        }

        // 4. Check resource usages
        if check_resources
            && usage.direct_count == 0
            && usage.transitive_count == 0
            && usage.xml_count == 0
        {
            // Get resources defined in the dependency module
            let mut res_stmt = conn.prepare(
                "SELECT r.type, r.name FROM resources r
                 JOIN modules m ON r.module_id = m.id
                 WHERE m.name = ?1
                 LIMIT 100",
            )?;
            let resources: Vec<(String, String)> = res_stmt
                .query_map(params![dep_name], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();

            // Check if these resources are used in the target module
            for (res_type, res_name) in &resources {
                let mut usage_stmt = conn.prepare(
                    "SELECT ru.usage_type FROM resource_usages ru
                     JOIN resources r ON ru.resource_id = r.id
                     WHERE r.type = ?1 AND r.name = ?2
                     AND ru.usage_file LIKE ?3",
                )?;
                let module_pattern = format!("{}%", module_path);
                let usages: Vec<String> = usage_stmt
                    .query_map(params![res_type, res_name, module_pattern], |row| {
                        row.get(0)
                    })?
                    .filter_map(|r| r.ok())
                    .collect();

                if !usages.is_empty() {
                    usage.resource_count += usages.len();
                    if usage.resource_usages.len() < 3 {
                        usage.resource_usages.push((
                            format!("@{}/{}", res_type, res_name),
                            usages.first().cloned().unwrap_or_default(),
                        ));
                    }
                }
            }
        }

        // Categorize the dependency
        let total_usage =
            usage.direct_count + usage.transitive_count + usage.xml_count + usage.resource_count;

        if total_usage == 0 {
            // Check if this is an api dependency (exported for consumers)
            if dep_kind == "api" {
                exported.push((dep_name.clone(), dep_path.clone(), dep_kind.clone()));
            } else {
                unused.push((dep_name.clone(), dep_path.clone(), dep_kind.clone()));
            }
        } else if usage.direct_count > 0 {
            used_direct.push((
                dep_name.clone(),
                dep_path.clone(),
                dep_kind.clone(),
                usage.direct_count,
            ));
        } else if usage.transitive_count > 0 {
            used_transitive.push((
                dep_name.clone(),
                dep_path.clone(),
                dep_kind.clone(),
                usage.transitive_count,
            ));
        } else if usage.xml_count > 0 {
            used_xml.push((
                dep_name.clone(),
                dep_path.clone(),
                dep_kind.clone(),
                usage.xml_count,
            ));
        } else if usage.resource_count > 0 {
            used_resources.push((
                dep_name.clone(),
                dep_path.clone(),
                dep_kind.clone(),
                usage.resource_count,
            ));
        }

        dep_usages.insert(dep_name.clone(), usage);
    }

    // Output results
    if verbose {
        println!("{}", "=== Direct Usage ===".cyan().bold());
        for (name, _, _, count) in &used_direct {
            let usage = dep_usages.get(name).unwrap();
            let symbols_str = if usage.direct_symbols.is_empty() {
                String::new()
            } else {
                format!(": {}", usage.direct_symbols.join(", "))
            };
            println!(
                "  {} {} - {} symbols{}",
                "✓".green(),
                name,
                count,
                symbols_str
            );
        }
        if used_direct.is_empty() {
            println!("  (none)");
        }

        if check_transitive {
            println!("\n{}", "=== Transitive Usage ===".cyan().bold());
            for (name, _, _, count) in &used_transitive {
                let usage = dep_usages.get(name).unwrap();
                println!("  {} {} - {} symbols", "✓".green(), name, count);
                for (via, symbols) in &usage.transitive_via {
                    println!("    └─ via {}: {}", via, symbols.join(", "));
                }
            }
            if used_transitive.is_empty() {
                println!("  (none)");
            }
        }

        if check_xml {
            println!("\n{}", "=== XML Usage ===".cyan().bold());
            for (name, _, _, count) in &used_xml {
                let usage = dep_usages.get(name).unwrap();
                println!("  {} {} - {} usages", "✓".green(), name, count);
                for (class, line) in &usage.xml_usages {
                    println!("    └─ {}:{}", class, line);
                }
            }
            if used_xml.is_empty() {
                println!("  (none)");
            }
        }

        if check_resources {
            println!("\n{}", "=== Resource Usage ===".cyan().bold());
            for (name, _, _, count) in &used_resources {
                let usage = dep_usages.get(name).unwrap();
                println!("  {} {} - {} usages", "✓".green(), name, count);
                for (res, usage_type) in &usage.resource_usages {
                    println!("    └─ {} ({})", res, usage_type);
                }
            }
            if used_resources.is_empty() {
                println!("  (none)");
            }
        }
    }

    // Exported (api deps not directly used but intentionally re-exported)
    if !exported.is_empty() {
        println!(
            "\n{}",
            "=== Exported (not directly used) ===".yellow().bold()
        );
        for (name, _path, _kind) in &exported {
            println!("  {} {} (api)", "⚡".yellow(), name);
            if verbose {
                // Find consumers who use this exported dep
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT m.name FROM module_deps md
                     JOIN modules m ON md.module_id = m.id
                     JOIN modules dep ON md.dep_module_id = dep.id
                     WHERE dep.name = ?1 AND m.name != ?2
                     LIMIT 5",
                )?;
                let consumers: Vec<String> = stmt
                    .query_map(params![name, module], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                if !consumers.is_empty() {
                    println!("    └─ used by: {}", consumers.join(", "));
                }
            }
        }
    }

    // Unused
    println!("\n{}", "=== Unused ===".red().bold());
    if !unused.is_empty() {
        for (name, _path, kind) in &unused {
            println!("  {} {} ({})", "✗".red(), name, kind);
            if verbose {
                println!("    - No direct imports");
                if check_transitive {
                    println!("    - No transitive usage");
                }
                if check_xml {
                    println!("    - No XML usage");
                }
                if check_resources {
                    println!("    - No resource usage");
                }
            }
        }
    } else {
        println!("  (none - all dependencies are used)");
    }

    println!("\n{}", "=== Summary ===".bold());
    let total_used =
        used_direct.len() + used_transitive.len() + used_xml.len() + used_resources.len();
    println!(
        "Total: {} unused, {} exported, {} used of {} dependencies",
        unused.len(),
        exported.len(),
        total_used,
        deps.len()
    );
    println!("  - Direct: {}", used_direct.len());
    if check_transitive {
        println!("  - Transitive: {}", used_transitive.len());
    }
    if check_xml {
        println!("  - XML: {}", used_xml.len());
    }
    if check_resources {
        println!("  - Resources: {}", used_resources.len());
    }
    if !exported.is_empty() {
        println!("  - Exported (api): {}", exported.len());
    }

    Ok(())
}

/// Get public symbols (classes, interfaces) from a module
fn get_module_public_symbols(
    conn: &Connection,
    root: &Path,
    module_path: &str,
) -> Result<Vec<String>> {
    let mut symbols = vec![];

    // First try to get from index
    let mut stmt = conn.prepare(
        "SELECT DISTINCT s.name FROM symbols s
         JOIN files f ON s.file_id = f.id
         WHERE f.path LIKE ?1 AND s.kind IN ('class', 'interface', 'object')
         LIMIT 100",
    )?;

    let pattern = format!("{}%", module_path);
    let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;

    for row in rows {
        if let Ok(name) = row {
            symbols.push(name);
        }
    }

    // If no symbols in index, try to find by scanning files
    if symbols.is_empty() {
        let module_dir = root.join(module_path);
        if module_dir.exists() {
            let class_re = Regex::new(
                r"(?m)^\s*(?:public\s+)?(?:abstract\s+)?(?:data\s+)?(?:class|interface|object)\s+(\w+)",
            )?;

            for entry in WalkDir::new(&module_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "kt" || ext == "java")
                        .unwrap_or(false)
                })
            {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for caps in class_re.captures_iter(&content) {
                        if let Some(name) = caps.get(1) {
                            symbols.push(name.as_str().to_string());
                        }
                    }
                }
                if symbols.len() >= 100 {
                    break;
                }
            }
        }
    }

    Ok(symbols)
}

// ── module-route ─────────────────────────────────────────────────────────────

/// A single edge in a dependency path.
#[derive(Debug, serde::Serialize, Clone)]
struct EdgeHop {
    from: String,
    to: String,
    kind: String,
}

/// One complete path from source to target module.
#[derive(Debug, serde::Serialize, Clone)]
struct RoutePath {
    hops: Vec<EdgeHop>,
    length: usize,
}

/// Full result envelope returned by `cmd_module_route`.
#[derive(Debug, serde::Serialize)]
struct ModuleRouteResult {
    from: String,
    to: String,
    paths: Vec<RoutePath>,
    count: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncation_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    empty_reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Search progress when DFS was used. Surfaces via JSON for tooling.
    #[serde(skip_serializing_if = "Option::is_none")]
    search_stats: Option<SearchStats>,
}

#[derive(Debug, serde::Serialize, Clone)]
struct SearchStats {
    nodes_visited: usize,
    edges_explored: usize,
    elapsed_ms: u64,
    max_depth_reached: usize,
    timeout_ms: u64,
    suggested_timeout_ms: Option<u64>,
}

fn render_json(result: &ModuleRouteResult) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(result)?);
    Ok(())
}

fn render_text(result: &ModuleRouteResult) {
    if let Some(reason) = &result.empty_reason {
        let hint = match reason.as_str() {
            "missing_module_from" => format!("Module '{}' not found in index.", result.from),
            "missing_module_to" => format!("Module '{}' not found in index.", result.to),
            "unreachable" => format!(
                "No dependency path from '{}' to '{}'.",
                result.from, result.to
            ),
            "self" => format!("'{}' depends on itself (trivial path).", result.from),
            "truncated_timeout" => {
                let progress = result
                    .search_stats
                    .as_ref()
                    .map(|s| {
                        let suggested = s
                            .suggested_timeout_ms
                            .map(|ms| format!(" Try --timeout-ms {}.", ms))
                            .unwrap_or_default();
                        format!(
                            " Explored {} edges across {} nodes (max depth {}) in {} ms before timeout ({} ms).{}",
                            s.edges_explored, s.nodes_visited, s.max_depth_reached, s.elapsed_ms, s.timeout_ms, suggested
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "Search from '{}' to '{}' timed out before finding any paths.{}",
                    result.from, result.to, progress
                )
            }
            "truncated_max_paths" => format!(
                "Hit max-paths limit before recording any complete path from '{}' to '{}'. Try --max-paths <larger> or --max-depth <smaller>.",
                result.from, result.to
            ),
            _ => format!("No path found (reason: {}).", reason),
        };
        println!("{}", hint.yellow());
        for w in &result.warnings {
            eprintln!("{}", format!("Warning: {}", w).yellow());
        }
        return;
    }

    let shortest = result.paths.first().map(|p| p.length).unwrap_or(0);
    println!(
        "{}",
        format!(
            "{} → {} ({} path{}, shortest = {} hop{})",
            result.from.cyan(),
            result.to.cyan(),
            result.count,
            if result.count == 1 { "" } else { "s" },
            shortest,
            if shortest == 1 { "" } else { "s" }
        )
        .bold()
    );

    for (i, path) in result.paths.iter().enumerate() {
        println!(
            "\n  Path {} ({} hop{}):",
            i + 1,
            path.length,
            if path.length == 1 { "" } else { "s" }
        );
        if path.hops.is_empty() {
            println!("    {} (same module)", result.from.cyan());
        }
        for hop in &path.hops {
            println!(
                "    {} → {} [{}]",
                hop.from.cyan(),
                hop.to.cyan(),
                hop.kind.dimmed()
            );
        }
    }

    if result.truncated {
        let reason = result.truncation_reason.as_deref().unwrap_or("limit");
        let detail = result
            .search_stats
            .as_ref()
            .map(|s| {
                let suggested = s
                    .suggested_timeout_ms
                    .map(|ms| format!(", try --timeout-ms {}", ms))
                    .unwrap_or_default();
                format!(
                    " — explored {} edges, {} nodes, max depth {} in {} ms{}",
                    s.edges_explored, s.nodes_visited, s.max_depth_reached, s.elapsed_ms, suggested
                )
            })
            .unwrap_or_default();
        println!("\n  {} (truncated: {}{})", "…".dimmed(), reason, detail);
    }

    for w in &result.warnings {
        eprintln!("{}", format!("Warning: {}", w).yellow());
    }
}

/// Escape a string for use as a Mermaid node label inside `[…]`.
///
/// The characters `[`, `]`, `(`, `)`, `{`, `}`, `|`, `"`, and newlines have
/// special meaning in Mermaid flowchart syntax and must be replaced or removed.
fn mermaid_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '[' => '⟦',
            ']' => '⟧',
            '(' => '❨',
            ')' => '❩',
            '{' => '❴',
            '}' => '❵',
            '|' => '∣',
            '"' => '\'',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

/// Escape a string for use inside a DOT double-quoted string.
///
/// DOT requires `"` → `\"` and `\n` → `\\n` inside quoted identifiers.
fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn render_mermaid(result: &ModuleRouteResult) {
    if result.paths.is_empty() {
        let reason = result.empty_reason.as_deref().unwrap_or("no_path");
        println!("```mermaid");
        println!("flowchart LR");
        println!("  %% No path: {}", reason);
        println!("```");
        return;
    }

    // Collect all unique node names and assign id aliases.
    let mut node_order: Vec<String> = Vec::new();
    let mut node_ids: HashMap<String, String> = HashMap::new();
    for path in &result.paths {
        for hop in &path.hops {
            for name in [&hop.from, &hop.to] {
                if !node_ids.contains_key(name.as_str()) {
                    let alias = format!("n{}", node_order.len());
                    node_ids.insert(name.clone(), alias);
                    node_order.push(name.clone());
                }
            }
        }
    }

    println!("```mermaid");
    println!("flowchart LR");
    for name in &node_order {
        let alias = &node_ids[name];
        // Mermaid label text inside `[]` must not contain `[](){}|"\n`.
        // Replace those chars with safe Unicode look-alikes / escape sequences.
        let safe = mermaid_escape(name);
        println!("  {}[{}]", alias, safe);
    }

    let mut seen_edges: HashSet<(String, String, String)> = HashSet::new();
    for path in &result.paths {
        for hop in &path.hops {
            let key = (hop.from.clone(), hop.to.clone(), hop.kind.clone());
            if seen_edges.contains(&key) {
                continue;
            }
            seen_edges.insert(key);
            let from_id = &node_ids[&hop.from];
            let to_id = &node_ids[&hop.to];
            // Omit edge label for "implementation" to reduce visual noise.
            if hop.kind == "implementation" {
                println!("  {} --> {}", from_id, to_id);
            } else {
                let safe_kind = mermaid_escape(&hop.kind);
                println!("  {} -->|{}| {}", from_id, safe_kind, to_id);
            }
        }
    }
    println!("```");
}

fn render_dot(result: &ModuleRouteResult) {
    println!("digraph module_route {{");
    println!("  rankdir=LR;");

    if result.paths.is_empty() {
        let reason = result.empty_reason.as_deref().unwrap_or("no_path");
        println!("  // No path: {}", reason);
        println!("}}");
        return;
    }

    // Collect unique node names.
    let mut node_set: HashSet<String> = HashSet::new();
    for path in &result.paths {
        for hop in &path.hops {
            node_set.insert(hop.from.clone());
            node_set.insert(hop.to.clone());
        }
    }
    let mut node_list: Vec<_> = node_set.iter().cloned().collect();
    node_list.sort();
    for name in &node_list {
        println!("  \"{}\";", dot_escape(name));
    }

    let mut seen_edges: HashSet<(String, String, String)> = HashSet::new();
    for path in &result.paths {
        for hop in &path.hops {
            let key = (hop.from.clone(), hop.to.clone(), hop.kind.clone());
            if seen_edges.contains(&key) {
                continue;
            }
            seen_edges.insert(key);
            println!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];",
                dot_escape(&hop.from),
                dot_escape(&hop.to),
                dot_escape(&hop.kind),
            );
        }
    }
    println!("}}");
}

/// Outcome of a `bfs_shortest` run. Distinguishes "no path exists" from
/// "we timed out before deciding" — the caller must NOT report `unreachable`
/// when the budget was exhausted.
enum BfsOutcome {
    Found(RoutePath),
    NotFound,
    TimedOut,
}

/// BFS from `from_id` to `to_id` respecting `kind_filter` and `max_depth`.
/// Stops as soon as `to_id` is first dequeued (single shortest path).
fn bfs_shortest(
    conn: &Connection,
    from_id: i64,
    to_id: i64,
    kind_filter: Option<&str>,
    max_depth: usize,
    deadline: Instant,
    timeout_ms: u64,
) -> Result<BfsOutcome> {
    // Queue entry: (node_id, current_depth)
    let mut queue: VecDeque<(i64, usize)> = VecDeque::new();
    // predecessor: node_id → (predecessor_id, edge_kind, node_name)
    let mut predecessor: HashMap<i64, (i64, String, String)> = HashMap::new();
    // name cache: id → name
    let mut names: HashMap<i64, String> = HashMap::new();

    // Seed the source name.
    let from_name: String = db::get_module_name(conn, from_id)?.unwrap_or_default();
    names.insert(from_id, from_name);

    queue.push_back((from_id, 0));
    let mut visited: HashSet<i64> = HashSet::new();
    visited.insert(from_id);

    while let Some((node_id, depth)) = queue.pop_front() {
        // Wall-clock guard checked per node (not per edge).
        if deadline.elapsed().as_millis() as u64 >= timeout_ms {
            return Ok(BfsOutcome::TimedOut);
        }

        if node_id == to_id {
            // Backtrack predecessor chain.
            return Ok(BfsOutcome::Found(build_path_from_predecessors(
                &predecessor,
                &names,
                from_id,
                to_id,
            )));
        }

        if depth >= max_depth {
            continue;
        }

        let edges = db::get_outgoing_edges_dedup(conn, node_id, kind_filter)?;
        for (dep_id, dep_name, edge_kind) in edges {
            if !visited.contains(&dep_id) {
                visited.insert(dep_id);
                let node_name = names.get(&node_id).cloned().unwrap_or_default();
                predecessor.insert(dep_id, (node_id, edge_kind, node_name));
                names.insert(dep_id, dep_name);
                queue.push_back((dep_id, depth + 1));
            }
        }
    }

    Ok(BfsOutcome::NotFound)
}

fn build_path_from_predecessors(
    predecessor: &HashMap<i64, (i64, String, String)>,
    names: &HashMap<i64, String>,
    from_id: i64,
    to_id: i64,
) -> RoutePath {
    let mut chain: Vec<(i64, String, String)> = Vec::new(); // (node_id, edge_kind, from_name)
    let mut cur = to_id;
    while cur != from_id {
        if let Some((prev_id, kind, from_name)) = predecessor.get(&cur) {
            chain.push((cur, kind.clone(), from_name.clone()));
            cur = *prev_id;
        } else {
            break;
        }
    }
    chain.reverse();

    let hops: Vec<EdgeHop> = chain
        .into_iter()
        .map(|(to_id_hop, kind, from_name)| EdgeHop {
            from: from_name,
            to: names.get(&to_id_hop).cloned().unwrap_or_default(),
            kind,
        })
        .collect();
    let length = hops.len();
    RoutePath { hops, length }
}

/// Frame for iterative DFS.
struct DfsFrame {
    node_id: i64,
    edges: Vec<(i64, String, String)>, // remaining outgoing edges to explore
    edge_idx: usize,
}

/// Statistics from a DFS traversal — used to give actionable hints when
/// the search hits a wall-clock or path-count cap.
#[derive(Debug, Clone, Default)]
pub struct DfsStats {
    pub nodes_visited: usize,
    pub edges_explored: usize,
    pub elapsed_ms: u64,
    pub max_depth_reached: usize,
}

/// Reverse BFS from `to_id`: returns the minimum number of hops needed
/// from each visited node to reach `to_id` (in forward graph), bounded by
/// `max_depth`. Nodes that cannot reach `to_id` within `max_depth` are
/// absent from the map. Used by `dfs_all_paths` to prune subtrees that
/// cannot lead to the target.
///
/// Returns `(distance_map, truncated_by_timeout)`. When the second element is
/// `true` the map is incomplete — the BFS was cut short by the wall-clock
/// deadline, so absence from the map does NOT mean "unreachable".
fn compute_reverse_distances(
    conn: &Connection,
    to_id: i64,
    kind_filter: Option<&str>,
    max_depth: usize,
    deadline: Instant,
    timeout_ms: u64,
) -> Result<(HashMap<i64, usize>, bool)> {
    let mut dist: HashMap<i64, usize> = HashMap::new();
    let mut queue: VecDeque<(i64, usize)> = VecDeque::new();
    let mut timed_out = false;
    dist.insert(to_id, 0);
    queue.push_back((to_id, 0));

    while let Some((node, d)) = queue.pop_front() {
        if deadline.elapsed().as_millis() as u64 >= timeout_ms {
            timed_out = true;
            break;
        }
        if d >= max_depth {
            continue;
        }
        let preds = db::get_incoming_edges_dedup(conn, node, kind_filter)?;
        for (pred_id, _name, _kind) in preds {
            if !dist.contains_key(&pred_id) {
                dist.insert(pred_id, d + 1);
                queue.push_back((pred_id, d + 1));
            }
        }
    }
    Ok((dist, timed_out))
}

/// DFS collecting all simple paths from `from_id` to `to_id`.
///
/// Pruning strategy: a reverse BFS from `to_id` computes the minimum hop
/// distance from every node to `to_id` (`dist_to`). During DFS we never
/// recurse into a child that is absent from `dist_to` (cannot reach the
/// target at all) or for which `current_depth + dist_to[child] > max_depth`
/// (cannot reach within budget). On large graphs (1k+ modules) this trims
/// 90%+ of decoy subtrees and lets DFS finish in milliseconds.
fn dfs_all_paths(
    conn: &Connection,
    from_id: i64,
    to_id: i64,
    kind_filter: Option<&str>,
    max_depth: usize,
    max_paths: usize,
    deadline: Instant,
    timeout_ms: u64,
) -> Result<(Vec<RoutePath>, bool, Option<String>, DfsStats)> {
    let mut results: Vec<RoutePath> = Vec::new();
    let mut truncated = false;
    let mut truncation_reason: Option<String> = None;
    let mut stats = DfsStats::default();

    // max_paths == 0 means "do not collect any path" — return truncated
    // immediately so we don't materialise one path before checking the cap.
    if max_paths == 0 {
        stats.elapsed_ms = deadline.elapsed().as_millis() as u64;
        return Ok((results, true, Some("max_paths".to_string()), stats));
    }

    // Reverse-BFS pruning map: node → min hops to to_id. Nodes that can't
    // reach to_id within max_depth are absent.
    let (dist_to, prune_timed_out) =
        compute_reverse_distances(conn, to_id, kind_filter, max_depth, deadline, timeout_ms)?;

    // When the reverse BFS was cut short we cannot trust absence from dist_to:
    // some reachable nodes may simply not have been visited yet.
    // Signal immediately rather than running DFS that would produce wrong "unreachable".
    if prune_timed_out {
        truncated = true;
        truncation_reason = Some("prune_timeout".to_string());
        stats.elapsed_ms = deadline.elapsed().as_millis() as u64;
        return Ok((results, truncated, truncation_reason, stats));
    }

    // If from_id can't reach to_id at all, return empty fast.
    if !dist_to.contains_key(&from_id) {
        stats.elapsed_ms = deadline.elapsed().as_millis() as u64;
        return Ok((results, truncated, truncation_reason, stats));
    }

    // Stack of frames; each frame owns its remaining edge list.
    let mut stack: Vec<DfsFrame> = Vec::new();
    // current_path tracks (node_id, from_name, edge_kind_into_this_node).
    let mut current_path: Vec<(i64, String, Option<String>)> = Vec::new();
    let mut on_path: HashSet<i64> = HashSet::new();

    // Name cache.
    let mut names: HashMap<i64, String> = HashMap::new();
    let from_name: String = db::get_module_name(conn, from_id)?.unwrap_or_default();
    names.insert(from_id, from_name.clone());

    // Push the root frame.
    let mut root_edges = db::get_outgoing_edges_dedup(conn, from_id, kind_filter)?;
    for (id, name, _) in &root_edges {
        names.insert(*id, name.clone());
    }
    // Reorder edges so any direct edge to `to_id` is processed first. Without
    // this, DFS may exhaust its timeout exploring siblings (alphabetically
    // earlier than the target) before ever recording the direct hit, and the
    // user gets a misleading "no path" result on a graph that obviously has one.
    root_edges.sort_by_key(|(id, _, _)| if *id == to_id { 0 } else { 1 });
    stack.push(DfsFrame {
        node_id: from_id,
        edges: root_edges,
        edge_idx: 0,
    });
    on_path.insert(from_id);
    current_path.push((from_id, from_name, None));

    'outer: loop {
        // Timeout check per frame operation.
        if deadline.elapsed().as_millis() as u64 >= timeout_ms {
            truncated = true;
            truncation_reason = Some("timeout".to_string());
            break;
        }

        let frame_depth = stack.len();
        let frame = match stack.last_mut() {
            Some(f) => f,
            None => break,
        };

        if frame.edge_idx >= frame.edges.len() {
            // All children of this frame explored — backtrack.
            on_path.remove(&frame.node_id);
            current_path.pop();
            stack.pop();
            continue;
        }

        let (child_id, child_name, edge_kind) = frame.edges[frame.edge_idx].clone();
        frame.edge_idx += 1;
        stats.edges_explored += 1;

        if on_path.contains(&child_id) {
            continue; // Cycle: skip.
        }

        // Reverse-BFS pruning: skip children that can't reach to_id within
        // remaining budget. frame_depth = hops already taken (edges in
        // current_path); +1 = this edge to child; +dist_to[child] = remaining
        // hops to to_id. Total path length must be ≤ max_depth.
        match dist_to.get(&child_id) {
            None => continue, // child cannot reach to_id at all.
            Some(&d_child) => {
                if frame_depth.saturating_add(d_child) > max_depth {
                    continue;
                }
            }
        }

        if child_id == to_id {
            // Found a path — materialise it.
            //
            // Layout of current_path:
            //   index 0  → (root_id,   root_name,   None)           ← no incoming edge
            //   index k  → (node_k_id, node_k_name, Some(kind_k-1→k)) ← kind of edge (k-1)→k
            //
            // So the outgoing edge kind for hop i→(i+1) is stored at
            // current_path[i+1].2 for intermediate hops, and `edge_kind` for
            // the final hop to `child_id` (= to_id).
            let mut hops: Vec<EdgeHop> = Vec::with_capacity(current_path.len());
            for i in 0..current_path.len() {
                let (_node_id, ref node_name, _) = current_path[i];
                let (next_name, kind) = if i + 1 < current_path.len() {
                    // Intermediate hop: next node is already on the path.
                    let next_id = current_path[i + 1].0;
                    let next_n = names.get(&next_id).cloned().unwrap_or_default();
                    // current_path[i+1].2 is the kind of edge i → i+1 (set when
                    // we pushed node i+1 onto the stack).
                    let k = current_path[i + 1].2.clone().unwrap_or_default();
                    (next_n, k)
                } else {
                    // Last hop: destination is child_id (= to_id), found in
                    // this iteration; edge_kind is the outgoing kind from node i.
                    (child_name.clone(), edge_kind.clone())
                };
                hops.push(EdgeHop {
                    from: node_name.clone(),
                    to: next_name,
                    kind,
                });
            }
            let length = hops.len();
            results.push(RoutePath { hops, length });

            if results.len() >= max_paths {
                truncated = true;
                truncation_reason = Some("max_paths".to_string());
                break 'outer;
            }
            continue; // Do not push to_id onto stack — stop here.
        }

        // Depth guard: current depth = stack.len() (number of frames already on stack = path length to frame.node_id)
        if frame_depth >= max_depth {
            continue;
        }

        // Push child onto stack.
        on_path.insert(child_id);
        names.insert(child_id, child_name.clone());
        stats.nodes_visited += 1;

        let mut child_edges = db::get_outgoing_edges_dedup(conn, child_id, kind_filter)?;
        for (id, name, _) in &child_edges {
            names.entry(*id).or_insert_with(|| name.clone());
        }
        child_edges.sort_by_key(|(id, _, _)| if *id == to_id { 0 } else { 1 });

        current_path.push((child_id, child_name, Some(edge_kind)));
        let new_depth = stack.len() + 1;
        if new_depth > stats.max_depth_reached {
            stats.max_depth_reached = new_depth;
        }
        stack.push(DfsFrame {
            node_id: child_id,
            edges: child_edges,
            edge_idx: 0,
        });
    }

    // Sort: shortest first, then lexicographic by hop names for determinism.
    results.sort_by(|a, b| {
        a.length.cmp(&b.length).then_with(|| {
            let a_key: Vec<_> = a.hops.iter().map(|h| h.to.as_str()).collect();
            let b_key: Vec<_> = b.hops.iter().map(|h| h.to.as_str()).collect();
            a_key.cmp(&b_key)
        })
    });

    stats.elapsed_ms = deadline.elapsed().as_millis() as u64;
    Ok((results, truncated, truncation_reason, stats))
}

/// Show dependency path(s) between two modules.
pub fn cmd_module_route(
    root: &Path,
    from: &str,
    to: &str,
    all: bool,
    max_paths: usize,
    max_depth: usize,
    timeout_ms: u64,
    via_kind: &str,
    format: &str,
) -> Result<()> {
    // Validate format early so JSON mode never emits ANSI.
    match format {
        "text" | "json" | "mermaid" | "dot" => {}
        _ => {
            // Unknown format: we cannot emit JSON because we don't know the
            // caller's intent, so emit a plain-text error to stderr.
            eprintln!(
                "{}",
                format!(
                    "Invalid --format '{}'. Use: text, json, mermaid, dot.",
                    format
                )
                .red()
            );
            return Ok(());
        }
    }

    // Validate via_kind.
    match via_kind {
        "api" | "implementation" | "all" => {}
        _ => {
            let msg = format!(
                "Invalid --via-kind '{}'. Use: api, implementation, all.",
                via_kind
            );
            if format == "json" {
                let result = ModuleRouteResult {
                    from: from.to_string(),
                    to: to.to_string(),
                    paths: vec![],
                    count: 0,
                    truncated: false,
                    truncation_reason: None,
                    empty_reason: Some("invalid_args".to_string()),
                    warnings: vec![msg],
                    search_stats: None,
                };
                return render_json(&result);
            }
            eprintln!("{}", msg.red());
            return Ok(());
        }
    }

    if !db::db_exists(root) {
        let msg = "Index not found. Run 'ast-index rebuild' first.";
        if format == "json" {
            let result = ModuleRouteResult {
                from: from.to_string(),
                to: to.to_string(),
                paths: vec![],
                count: 0,
                truncated: false,
                truncation_reason: None,
                empty_reason: Some("index_missing".to_string()),
                warnings: vec![],
                search_stats: None,
            };
            return render_json(&result);
        }
        eprintln!("{}", msg.red());
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;

    // Check module_deps populated.
    if db::count_module_deps(&conn)? == 0 {
        let msg = "Module dependencies not indexed. Run 'ast-index rebuild'.";
        if format == "json" {
            let result = ModuleRouteResult {
                from: from.to_string(),
                to: to.to_string(),
                paths: vec![],
                count: 0,
                truncated: false,
                truncation_reason: None,
                empty_reason: Some("not_indexed".to_string()),
                warnings: vec![],
                search_stats: None,
            };
            return render_json(&result);
        }
        eprintln!("{}", msg.yellow());
        return Ok(());
    }

    // Multi-root warning.
    let extra_roots = db::get_extra_roots(&conn)?;
    if !extra_roots.is_empty() {
        eprintln!(
            "{}",
            "Multi-root project detected; module-route v1 only walks the primary root graph."
                .yellow()
        );
    }

    // Staleness warning.
    let mut warnings: Vec<String> = Vec::new();
    if let Ok(Some((indexed_at, updated_at))) = db::get_modules_index_freshness(&conn) {
        if updated_at > indexed_at {
            warnings.push("index_may_be_stale".to_string());
        }
    }

    // Resolve module ids.
    let from_id = db::find_module_id_by_name(&conn, from)?;
    let to_id = db::find_module_id_by_name(&conn, to)?;

    let kind_filter: Option<&str> = if via_kind == "all" {
        None
    } else {
        Some(via_kind)
    };
    let deadline = Instant::now();

    // Self-query: when from == to, check whether a real self-edge exists in
    // the DB. If it does, traverse it as a proper 1-hop cycle path. Otherwise
    // return empty with empty_reason="self" so callers can distinguish.
    if let (Some(fid), Some(_tid)) = (from_id, to_id) {
        if fid == _tid {
            if let Some(real_kind) = db::get_module_self_edge_kind(&conn, fid, kind_filter)? {
                // Real self-loop: surface the actual dep_kind from the DB,
                // not a hardcoded default.
                let name = db::get_module_name(&conn, fid)?.unwrap_or_else(|| from.to_string());
                let hop = EdgeHop {
                    from: name.clone(),
                    to: name,
                    kind: real_kind,
                };
                let result = ModuleRouteResult {
                    from: from.to_string(),
                    to: to.to_string(),
                    paths: vec![RoutePath {
                        hops: vec![hop],
                        length: 1,
                    }],
                    count: 1,
                    truncated: false,
                    truncation_reason: None,
                    empty_reason: None,
                    warnings,
                    search_stats: None,
                };
                return dispatch_render(format, &result);
            }
            // No self-edge: trivial "same module" answer.
            let result = ModuleRouteResult {
                from: from.to_string(),
                to: to.to_string(),
                paths: vec![],
                count: 0,
                truncated: false,
                truncation_reason: None,
                empty_reason: Some("self".to_string()),
                warnings,
                search_stats: None,
            };
            return dispatch_render(format, &result);
        }
    }

    // Missing module handling.
    let (fid, tid) = match (from_id, to_id) {
        (None, _) => {
            let result = ModuleRouteResult {
                from: from.to_string(),
                to: to.to_string(),
                paths: vec![],
                count: 0,
                truncated: false,
                truncation_reason: None,
                empty_reason: Some("missing_module_from".to_string()),
                warnings,
                search_stats: None,
            };
            return dispatch_render(format, &result);
        }
        (_, None) => {
            let result = ModuleRouteResult {
                from: from.to_string(),
                to: to.to_string(),
                paths: vec![],
                count: 0,
                truncated: false,
                truncation_reason: None,
                empty_reason: Some("missing_module_to".to_string()),
                warnings,
                search_stats: None,
            };
            return dispatch_render(format, &result);
        }
        (Some(f), Some(t)) => (f, t),
    };

    // Run BFS/DFS.
    let (paths, truncated, truncation_reason, empty_reason, search_stats) = if all {
        let (paths, truncated, trunc_reason, dfs_stats) = dfs_all_paths(
            &conn,
            fid,
            tid,
            kind_filter,
            max_depth,
            max_paths,
            deadline,
            timeout_ms,
        )?;
        let reason = if paths.is_empty() {
            // Truncated (timeout / prune_timeout / max_paths) wins over
            // reachability — saying "no path" when the search was cut short
            // is a lie.
            if truncated {
                // prune_timeout gets its own value so callers can distinguish
                // "we ran DFS but timed out" from "the pruning phase itself
                // was too slow to finish".
                let tag = trunc_reason.as_deref().unwrap_or("limit");
                Some(format!("truncated_{}", tag))
            } else {
                let reachable = if kind_filter.is_some() {
                    matches!(
                        bfs_shortest(&conn, fid, tid, None, max_depth, deadline, timeout_ms)?,
                        BfsOutcome::Found(_)
                    )
                } else {
                    false
                };
                if reachable {
                    Some("kind_filter".to_string())
                } else {
                    Some("unreachable".to_string())
                }
            }
        } else {
            None
        };
        // Suggest a higher --timeout-ms for both DFS timeout and prune timeout.
        let suggested_timeout_ms = if truncated
            && matches!(
                trunc_reason.as_deref(),
                Some("timeout") | Some("prune_timeout")
            ) {
            // Heuristic: 2× current, rounded up to next second. Capped at 60s
            // to keep the suggestion sane on pathological graphs.
            let bumped = (timeout_ms.saturating_mul(2)).max(timeout_ms + 1000);
            Some(bumped.min(60_000))
        } else {
            None
        };
        let stats = SearchStats {
            nodes_visited: dfs_stats.nodes_visited,
            edges_explored: dfs_stats.edges_explored,
            elapsed_ms: dfs_stats.elapsed_ms,
            max_depth_reached: dfs_stats.max_depth_reached,
            timeout_ms,
            suggested_timeout_ms,
        };
        (paths, truncated, trunc_reason, reason, Some(stats))
    } else {
        let outcome = bfs_shortest(
            &conn,
            fid,
            tid,
            kind_filter,
            max_depth,
            deadline,
            timeout_ms,
        )?;
        match outcome {
            BfsOutcome::Found(p) => (vec![p], false, None, None, None),
            BfsOutcome::TimedOut => {
                // Shortest-mode timeout must NOT collapse to "unreachable" —
                // we don't know whether a path exists. Report as truncated,
                // mirroring the --all behaviour, so callers can retry with a
                // larger --timeout-ms.
                let suggested_timeout_ms = {
                    let bumped = (timeout_ms.saturating_mul(2)).max(timeout_ms + 1000);
                    Some(bumped.min(60_000))
                };
                let stats = SearchStats {
                    nodes_visited: 0,
                    edges_explored: 0,
                    elapsed_ms: deadline.elapsed().as_millis() as u64,
                    max_depth_reached: 0,
                    timeout_ms,
                    suggested_timeout_ms,
                };
                (
                    vec![],
                    true,
                    Some("timeout".to_string()),
                    Some("truncated_timeout".to_string()),
                    Some(stats),
                )
            }
            BfsOutcome::NotFound => {
                let reason = if kind_filter.is_some() {
                    // Check without filter; reuse the original deadline so the
                    // total work stays within the user-specified budget.
                    let outcome_unfiltered =
                        bfs_shortest(&conn, fid, tid, None, max_depth, deadline, timeout_ms)?;
                    match outcome_unfiltered {
                        BfsOutcome::Found(_) => Some("kind_filter".to_string()),
                        // Treat a follow-up timeout as unreachable here: the
                        // primary attempt already returned NotFound, so the
                        // caller knows the kind-filtered path is absent; the
                        // unfiltered probe is best-effort.
                        BfsOutcome::NotFound | BfsOutcome::TimedOut => {
                            Some("unreachable".to_string())
                        }
                    }
                } else {
                    Some("unreachable".to_string())
                };
                (vec![], false, None, reason, None)
            }
        }
    };

    let count = paths.len();
    let result = ModuleRouteResult {
        from: from.to_string(),
        to: to.to_string(),
        paths,
        count,
        truncated,
        truncation_reason,
        empty_reason,
        warnings,
        search_stats,
    };

    dispatch_render(format, &result)
}

fn dispatch_render(format: &str, result: &ModuleRouteResult) -> Result<()> {
    match format {
        "json" => render_json(result),
        "mermaid" => {
            render_mermaid(result);
            Ok(())
        }
        "dot" => {
            render_dot(result);
            Ok(())
        }
        _ => {
            render_text(result);
            Ok(())
        }
    }
}

/// Check if any symbols from a dependency are used in the target module (index-based)
///
/// Uses the refs table for fast lookups instead of scanning files on disk.
fn count_symbols_used_in_module(
    conn: &Connection,
    dep_symbols: &[String],
    module_path: &str,
) -> Result<(usize, Vec<String>)> {
    let module_pattern = format!("{}%", module_path);
    let mut used_count = 0;
    let mut used_names = Vec::new();

    let mut stmt = conn.prepare_cached(
        "SELECT COUNT(*) FROM refs r
         JOIN files f ON r.file_id = f.id
         WHERE r.name = ?1 AND f.path LIKE ?2",
    )?;

    for symbol in dep_symbols {
        let count: i64 = stmt
            .query_row(params![symbol, &module_pattern], |row| row.get(0))
            .unwrap_or(0);
        if count > 0 {
            used_count += 1;
            if used_names.len() < 3 {
                used_names.push(symbol.clone());
            }
        }
    }

    Ok((used_count, used_names))
}
