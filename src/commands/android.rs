//! Android-specific commands
//!
//! Commands for working with Android codebases:
//! - xml_usages: Find XML usages of a class (layouts, views)
//! - resource_usages: Find Android resource usages (drawables, strings, etc.)

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use rusqlite::params;

use crate::db;

/// Find XML usages of a class (layouts, views)
pub fn cmd_xml_usages(root: &Path, class_name: &str, module_filter: Option<&str>) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if XML usages are indexed
    let xml_count: i64 = conn.query_row("SELECT COUNT(*) FROM xml_usages", [], |row| row.get(0))?;
    if xml_count == 0 {
        println!("{}", "XML usages not indexed. Run 'ast-index rebuild' first.".yellow());
        return Ok(());
    }

    // Search for class in XML usages
    let pattern = format!("%{}%", class_name);

    let results: Vec<(String, String, i64, String, Option<String>)> = if let Some(module) = module_filter {
        let mut stmt = conn.prepare(
            "SELECT m.name, x.file_path, x.line, x.class_name, x.element_id
             FROM xml_usages x
             JOIN modules m ON x.module_id = m.id
             WHERE x.class_name LIKE ?1 AND m.name = ?2
             ORDER BY m.name, x.file_path, x.line"
        )?;
        let rows = stmt.query_map(params![pattern, module], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn.prepare(
            "SELECT m.name, x.file_path, x.line, x.class_name, x.element_id
             FROM xml_usages x
             JOIN modules m ON x.module_id = m.id
             WHERE x.class_name LIKE ?1
             ORDER BY m.name, x.file_path, x.line
             LIMIT 100"
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };

    println!("{}", format!("XML usages of '{}' ({}):", class_name, results.len()).bold());

    // Group by module
    let mut by_module: HashMap<String, Vec<(String, i64, String, Option<String>)>> = HashMap::new();
    for (module, file, line, class, element_id) in results {
        by_module.entry(module).or_default().push((file, line, class, element_id));
    }

    for (module, usages) in &by_module {
        println!("\n{}:", module.cyan());
        for (file, line, class, element_id) in usages {
            let id_str = element_id.as_ref()
                .map(|id| format!(" ({})", id))
                .unwrap_or_default();
            println!("  {}:{}", file, line);
            println!("    <{} ...{} />", class, id_str);
        }
    }

    if by_module.is_empty() {
        println!("  No XML usages found.");
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Find Android resource usages (drawables, strings, colors, etc.)
pub fn cmd_resource_usages(
    root: &Path,
    resource: &str,
    module_filter: Option<&str>,
    type_filter: Option<&str>,
    show_unused: bool,
) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!("{}", "Index not found. Run 'ast-index rebuild' first.".red());
        return Ok(());
    }

    let conn = db::open_db(root)?;

    // Check if resources are indexed
    let res_count: i64 = conn.query_row("SELECT COUNT(*) FROM resources", [], |row| row.get(0))?;
    if res_count == 0 {
        println!("{}", "Resources not indexed. Run 'ast-index rebuild' first.".yellow());
        return Ok(());
    }

    if show_unused {
        // Show unused resources in the module
        let module = module_filter.unwrap_or("");
        if module.is_empty() {
            println!("{}", "Please specify --module to find unused resources.".yellow());
            return Ok(());
        }
    } else if resource.is_empty() {
        println!("{}", "Please specify a resource name (e.g., @drawable/ic_payment or use --unused).".yellow());
        return Ok(());
    }

    if show_unused {
        let module = module_filter.unwrap_or("");
        println!("{}", format!("Unused resources in '{}':", module).bold());

        // Find resources defined in module that have no usages
        let mut stmt = conn.prepare(
            "SELECT r.type, r.name, r.file_path
             FROM resources r
             JOIN modules m ON r.module_id = m.id
             LEFT JOIN resource_usages ru ON r.id = ru.resource_id
             WHERE m.name = ?1 AND ru.id IS NULL
             ORDER BY r.type, r.name"
        )?;

        let unused: Vec<(String, String, String)> = stmt
            .query_map(params![module], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();

        // Group by type
        let mut by_type: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (rtype, name, path) in unused {
            if type_filter.map(|t| t == rtype).unwrap_or(true) {
                by_type.entry(rtype).or_default().push((name, path));
            }
        }

        let mut total = 0;
        for (rtype, items) in &by_type {
            println!("\n{} ({}):", rtype.cyan(), items.len());
            for (name, path) in items.iter().take(10) {
                println!("  {} @{}/{}", "⚠".yellow(), rtype, name);
                println!("    defined in: {}", path);
            }
            if items.len() > 10 {
                println!("  ... and {} more", items.len() - 10);
            }
            total += items.len();
        }

        println!("\n{}", format!("Total unused: {} resources", total).bold());

    } else {
        // Parse resource reference (e.g., @drawable/ic_payment or R.string.app_name)
        let (res_type, res_name) = parse_resource_reference(resource);

        let res_type = type_filter.unwrap_or(&res_type);

        println!("{}", format!("Usages of '@{}/{}':", res_type, res_name).bold());

        // Find resource usages
        let results: Vec<(String, i64, String)> = if let Some(module) = module_filter {
            let mut stmt = conn.prepare(
                "SELECT ru.usage_file, ru.usage_line, ru.usage_type
                 FROM resource_usages ru
                 JOIN resources r ON ru.resource_id = r.id
                 WHERE r.type = ?1 AND r.name = ?2 AND ru.usage_file LIKE ?3
                 ORDER BY ru.usage_file, ru.usage_line
                 LIMIT 100"
            )?;
            let module_pattern = format!("%{}%", module);
            let rows = stmt.query_map(params![res_type, res_name, module_pattern], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn.prepare(
                "SELECT ru.usage_file, ru.usage_line, ru.usage_type
                 FROM resource_usages ru
                 JOIN resources r ON ru.resource_id = r.id
                 WHERE r.type = ?1 AND r.name = ?2
                 ORDER BY ru.usage_file, ru.usage_line
                 LIMIT 100"
            )?;
            let rows = stmt.query_map(params![res_type, res_name], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.filter_map(|r| r.ok()).collect()
        };

        // Group by usage type
        let code_usages: Vec<_> = results.iter().filter(|(_, _, t)| t == "code").collect();
        let xml_usages: Vec<_> = results.iter().filter(|(_, _, t)| t == "xml").collect();

        if !code_usages.is_empty() {
            println!("\n{} ({}):", "Kotlin/Java".cyan(), code_usages.len());
            for (file, line, _) in code_usages.iter().take(10) {
                println!("  {}:{}", file, line);
            }
            if code_usages.len() > 10 {
                println!("  ... and {} more", code_usages.len() - 10);
            }
        }

        if !xml_usages.is_empty() {
            println!("\n{} ({}):", "XML".cyan(), xml_usages.len());
            for (file, line, _) in xml_usages.iter().take(10) {
                println!("  {}:{}", file, line);
            }
            if xml_usages.len() > 10 {
                println!("  ... and {} more", xml_usages.len() - 10);
            }
        }

        if results.is_empty() {
            println!("  No usages found.");
        } else {
            println!("\n{}", format!("Total: {} usages", results.len()).bold());
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Parse resource reference like @drawable/ic_name or R.string.name
fn parse_resource_reference(resource: &str) -> (String, String) {
    // Format: @type/name
    if resource.starts_with('@') {
        let parts: Vec<&str> = resource[1..].splitn(2, '/').collect();
        if parts.len() == 2 {
            return (parts[0].to_string(), parts[1].to_string());
        }
    }

    // Format: R.type.name
    if resource.starts_with("R.") {
        let parts: Vec<&str> = resource[2..].splitn(2, '.').collect();
        if parts.len() == 2 {
            return (parts[0].to_string(), parts[1].to_string());
        }
    }

    // Assume it's just a name, try to guess type from prefix
    let resource = resource.trim_start_matches('@');
    if resource.starts_with("ic_") || resource.starts_with("img_") {
        return ("drawable".to_string(), resource.to_string());
    }
    if resource.starts_with("color_") {
        return ("color".to_string(), resource.to_string());
    }

    // Default: assume it's a string resource
    ("string".to_string(), resource.to_string())
}
