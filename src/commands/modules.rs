//! Module-related commands
//!
//! Commands for working with project modules:
//! - module: Find modules by pattern
//! - deps: Show module dependencies
//! - dependents: Show modules that depend on a module
//! - unused_deps: Find unused dependencies

use std::collections::HashMap;
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
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show module dependencies
pub fn cmd_deps(root: &Path, module: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;

    if dep_count == 0 {
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
    let impl_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k == "implementation").collect();
    let other_deps: Vec<_> = deps.iter().filter(|(_, _, k)| k != "api" && k != "implementation").collect();

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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Show modules that depend on a module
pub fn cmd_dependents(root: &Path, module: &str) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;

    if dep_count == 0 {
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
    let impl_deps: Vec<_> = dependents.iter().filter(|(_, _, k)| k == "implementation").collect();
    let other_deps: Vec<_> = dependents.iter().filter(|(_, _, k)| k != "api" && k != "implementation").collect();

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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
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
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if module deps are indexed
    let dep_count: i64 = conn.query_row("SELECT COUNT(*) FROM module_deps", [], |row| row.get(0))?;
    if dep_count == 0 {
        println!("{}", "Module dependencies not indexed. Run 'ast-index rebuild' first.".yellow());
        return Ok(());
    }

    // Get module id and path
    let module_info: Option<(i64, String)> = conn.query_row(
        "SELECT id, path FROM modules WHERE name = ?1",
        params![module],
        |row| Ok((row.get(0)?, row.get(1)?))
    ).ok();

    let (module_id, module_path) = match module_info {
        Some((id, p)) => (id, p),
        None => {
            println!("{}", format!("Module '{}' not found in index.", module).red());
            return Ok(());
        }
    };

    // Get all dependencies
    let deps = indexer::get_module_deps(&conn, module)?;

    if deps.is_empty() {
        println!("{}", format!("Module '{}' has no dependencies.", module).yellow());
        return Ok(());
    }

    println!("{}", format!("Analyzing {} dependencies of '{}'...", deps.len(), module).bold());
    if check_transitive || check_xml || check_resources {
        let checks: Vec<&str> = [
            if check_transitive { Some("transitive") } else { None },
            if check_xml { Some("XML") } else { None },
            if check_resources { Some("resources") } else { None },
        ].into_iter().flatten().collect();
        println!("  Checking: direct imports + {}\n", checks.join(", "));
    } else {
        println!("  Checking: direct imports only (strict mode)\n");
    }

    let module_dir = root.join(&module_path);

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

        // 1. Check direct usage
        let dep_symbols = get_module_public_symbols(&conn, root, dep_path)?;

        for symbol in &dep_symbols {
            if is_symbol_used_in_module(root, &module_dir, symbol)? {
                usage.direct_count += 1;
                if usage.direct_symbols.len() < 3 {
                    usage.direct_symbols.push(symbol.clone());
                }
            }
        }

        // 2. Check transitive usage (via api dependencies)
        if check_transitive && usage.direct_count == 0 {
            // Get transitive paths for this dependency
            let mut stmt = conn.prepare(
                "SELECT td.path FROM transitive_deps td
                 JOIN modules m ON td.dependency_id = m.id
                 WHERE td.module_id = ?1 AND m.name = ?2 AND td.depth > 1"
            )?;

            let paths: Vec<String> = stmt
                .query_map(params![module_id, dep_name], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            for path in paths {
                // Parse the path (e.g., "A -> B -> C")
                let parts: Vec<&str> = path.split(" -> ").collect();
                if parts.len() >= 2 {
                    let via_module = parts[1];
                    // Check if any symbols from dep are re-exported via the intermediate module
                    for symbol in &dep_symbols {
                        if is_symbol_used_in_module(root, &module_dir, symbol)? {
                            usage.transitive_count += 1;
                            let entry = usage.transitive_via.iter_mut()
                                .find(|(m, _)| m == via_module);
                            if let Some((_, symbols)) = entry {
                                if symbols.len() < 3 {
                                    symbols.push(symbol.clone());
                                }
                            } else {
                                usage.transitive_via.push((via_module.to_string(), vec![symbol.clone()]));
                            }
                            break; // Found transitive usage
                        }
                    }
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
                 LIMIT 50"
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
                     WHERE m.id = ?1 AND x.class_name LIKE ?2"
                )?;
                let class_pattern = format!("%{}", class_name);
                let xml_results: Vec<(String, i64)> = xml_stmt
                    .query_map(params![module_id, class_pattern], |row| Ok((row.get(0)?, row.get(1)?)))?
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
        if check_resources && usage.direct_count == 0 && usage.transitive_count == 0 && usage.xml_count == 0 {
            // Get resources defined in the dependency module
            let mut res_stmt = conn.prepare(
                "SELECT r.type, r.name FROM resources r
                 JOIN modules m ON r.module_id = m.id
                 WHERE m.name = ?1
                 LIMIT 100"
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
                     AND ru.usage_file LIKE ?3"
                )?;
                let module_pattern = format!("{}%", module_path);
                let usages: Vec<String> = usage_stmt
                    .query_map(params![res_type, res_name, module_pattern], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();

                if !usages.is_empty() {
                    usage.resource_count += usages.len();
                    if usage.resource_usages.len() < 3 {
                        usage.resource_usages.push((
                            format!("@{}/{}", res_type, res_name),
                            usages.first().cloned().unwrap_or_default()
                        ));
                    }
                }
            }
        }

        // Categorize the dependency
        let total_usage = usage.direct_count + usage.transitive_count + usage.xml_count + usage.resource_count;

        if total_usage == 0 {
            // Check if this is an api dependency (exported for consumers)
            if dep_kind == "api" {
                exported.push((dep_name.clone(), dep_path.clone(), dep_kind.clone()));
            } else {
                unused.push((dep_name.clone(), dep_path.clone(), dep_kind.clone()));
            }
        } else if usage.direct_count > 0 {
            used_direct.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.direct_count));
        } else if usage.transitive_count > 0 {
            used_transitive.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.transitive_count));
        } else if usage.xml_count > 0 {
            used_xml.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.xml_count));
        } else if usage.resource_count > 0 {
            used_resources.push((dep_name.clone(), dep_path.clone(), dep_kind.clone(), usage.resource_count));
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
            println!("  {} {} - {} symbols{}", "✓".green(), name, count, symbols_str);
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
        println!("\n{}", "=== Exported (not directly used) ===".yellow().bold());
        for (name, _path, _kind) in &exported {
            println!("  {} {} (api)", "⚡".yellow(), name);
            if verbose {
                // Find consumers who use this exported dep
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT m.name FROM module_deps md
                     JOIN modules m ON md.module_id = m.id
                     JOIN modules dep ON md.dep_module_id = dep.id
                     WHERE dep.name = ?1 AND m.name != ?2
                     LIMIT 5"
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
                if check_transitive { println!("    - No transitive usage"); }
                if check_xml { println!("    - No XML usage"); }
                if check_resources { println!("    - No resource usage"); }
            }
        }
    } else {
        println!("  (none - all dependencies are used)");
    }

    println!("\n{}", "=== Summary ===".bold());
    let total_used = used_direct.len() + used_transitive.len() + used_xml.len() + used_resources.len();
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

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Get public symbols (classes, interfaces) from a module
fn get_module_public_symbols(conn: &Connection, root: &Path, module_path: &str) -> Result<Vec<String>> {
    let mut symbols = vec![];

    // First try to get from index
    let mut stmt = conn.prepare(
        "SELECT DISTINCT s.name FROM symbols s
         JOIN files f ON s.file_id = f.id
         WHERE f.path LIKE ?1 AND s.kind IN ('class', 'interface', 'object')
         LIMIT 100"
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
            let class_re = Regex::new(r"(?m)^\s*(?:public\s+)?(?:abstract\s+)?(?:data\s+)?(?:class|interface|object)\s+(\w+)")?;

            for entry in WalkDir::new(&module_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension()
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

/// Check if a symbol is used in the module directory
fn is_symbol_used_in_module(_root: &Path, module_dir: &Path, symbol: &str) -> Result<bool> {
    if !module_dir.exists() {
        return Ok(false);
    }

    let mut found = false;
    let class_pat = format!("class {}", symbol);
    let iface_pat = format!("interface {}", symbol);
    let obj_pat = format!("object {}", symbol);

    for entry in WalkDir::new(module_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension()
                .map(|ext| ext == "kt" || ext == "java")
                .unwrap_or(false)
        })
    {
        if found { break; }

        let path = entry.path();
        if let Ok(content) = std::fs::read_to_string(path) {
            // Skip if file is in the same module as the symbol definition
            // (we want usages, not definitions)
            if content.contains(&class_pat)
                || content.contains(&iface_pat)
                || content.contains(&obj_pat) {
                continue;
            }

            // Check for import or direct usage
            if content.contains(symbol) {
                found = true;
            }
        }
    }

    Ok(found)
}
