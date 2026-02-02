//! Index management commands
//!
//! Commands for managing the code index:
//! - init: Initialize an empty index
//! - rebuild: Rebuild the index (full or partial)
//! - update: Incrementally update the index
//! - stats: Show index statistics

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;

use crate::db;
use crate::indexer;

/// Initialize an empty index
pub fn cmd_init(root: &Path) -> Result<()> {
    let start = Instant::now();

    if db::db_exists(root) {
        println!("{}", "Index already exists. Use 'rebuild' to reindex.".yellow());
        return Ok(());
    }

    let conn = db::open_db(root)?;
    db::init_db(&conn)?;

    println!("{}", "Initialized empty index.".green());
    println!("Run 'ast-index rebuild' to build the index.");

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Rebuild the index (full or partial)
pub fn cmd_rebuild(root: &Path, index_type: &str, index_deps: bool, no_ignore: bool) -> Result<()> {
    let start = Instant::now();

    // Delete DB file entirely to avoid WAL hangs
    db::delete_db(root)?;

    // Remove old kotlin-index cache dir entirely
    db::cleanup_legacy_cache();

    let mut conn = db::open_db(root)?;
    db::init_db(&conn)?;

    // Store no_ignore setting in database metadata
    if no_ignore {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('no_ignore', '1')",
            [],
        ).ok();
        println!("{}", "Including gitignored files (build/, etc.)...".yellow());
    }

    // Detect project type
    let project_type = indexer::detect_project_type(root);
    let is_ios = matches!(project_type, indexer::ProjectType::IOS | indexer::ProjectType::Mixed);
    let is_android = matches!(project_type, indexer::ProjectType::Android | indexer::ProjectType::Mixed);

    match index_type {
        "all" => {
            println!("{}", "Rebuilding full index...".cyan());
            let file_count = indexer::index_directory(&mut conn, root, true, no_ignore)?;
            let module_count = indexer::index_modules(&conn, root)?;

            // Index CocoaPods/Carthage for iOS
            if is_ios {
                let pkg_count = indexer::index_ios_package_managers(&conn, root, true)?;
                if pkg_count > 0 {
                    println!("{}", format!("Indexed {} CocoaPods/Carthage deps", pkg_count).dimmed());
                }
            }

            let mut dep_count = 0;
            let mut trans_count = 0;
            if index_deps && is_android {
                println!("{}", "Indexing module dependencies...".cyan());
                dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
                trans_count = indexer::build_transitive_deps(&mut conn, true)?;
            }

            // Android-specific: XML layouts and resources
            let mut xml_count = 0;
            let mut res_count = 0;
            let mut res_usage_count = 0;
            if is_android {
                println!("{}", "Indexing XML layouts...".cyan());
                xml_count = indexer::index_xml_usages(&mut conn, root, true)?;

                println!("{}", "Indexing resources...".cyan());
                let (rc, ruc) = indexer::index_resources(&mut conn, root, true)?;
                res_count = rc;
                res_usage_count = ruc;
            }

            // iOS-specific: storyboards and assets
            let mut sb_count = 0;
            let mut asset_count = 0;
            let mut asset_usage_count = 0;
            if is_ios {
                println!("{}", "Indexing storyboards/xibs...".cyan());
                sb_count = indexer::index_storyboard_usages(&mut conn, root, true)?;

                println!("{}", "Indexing iOS assets...".cyan());
                let (ac, auc) = indexer::index_ios_assets(&mut conn, root, true)?;
                asset_count = ac;
                asset_usage_count = auc;
            }

            // Print summary based on project type
            if is_android && is_ios {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files, {} modules, {} deps, {} XML usages, {} resources, {} storyboard usages, {} assets",
                        file_count, module_count, dep_count, xml_count, res_count, sb_count, asset_count
                    ).green()
                );
            } else if is_ios {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files, {} modules, {} storyboard usages, {} assets ({} usages)",
                        file_count, module_count, sb_count, asset_count, asset_usage_count
                    ).green()
                );
            } else {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files, {} modules, {} deps, {} transitive, {} XML usages, {} resources ({} usages)",
                        file_count, module_count, dep_count, trans_count, xml_count, res_count, res_usage_count
                    ).green()
                );
            }
        }
        "files" | "symbols" => {
            println!("{}", "Rebuilding symbols index...".cyan());
            conn.execute("DELETE FROM symbols", [])?;
            conn.execute("DELETE FROM files", [])?;
            let file_count = indexer::index_directory(&mut conn, root, true, no_ignore)?;
            println!("{}", format!("Indexed {} files", file_count).green());
        }
        "modules" => {
            println!("{}", "Rebuilding modules index...".cyan());
            conn.execute("DELETE FROM module_deps", [])?;
            conn.execute("DELETE FROM modules", [])?;
            let module_count = indexer::index_modules(&conn, root)?;

            if index_deps {
                println!("{}", "Indexing module dependencies...".cyan());
                let dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
                println!(
                    "{}",
                    format!("Indexed {} modules, {} dependencies", module_count, dep_count).green()
                );
            } else {
                println!("{}", format!("Indexed {} modules", module_count).green());
            }
        }
        "deps" => {
            println!("{}", "Indexing module dependencies...".cyan());
            let dep_count = indexer::index_module_dependencies(&mut conn, root, true)?;
            println!("{}", format!("Indexed {} dependencies", dep_count).green());
        }
        _ => {
            println!("{}", format!("Unknown index type: {}", index_type).red());
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Incrementally update the index
pub fn cmd_update(root: &Path) -> Result<()> {
    let start = Instant::now();

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index init' first.".red()
        );
        return Ok(());
    }

    let mut conn = db::open_db(root)?;

    println!("{}", "Checking for changes...".cyan());
    let (updated, changed, deleted) = indexer::update_directory_incremental(&mut conn, root, true)?;

    if updated == 0 && deleted == 0 {
        println!("{}", "Index is up to date.".green());
    } else {
        println!(
            "{}",
            format!(
                "Updated: {} files ({} changed, {} deleted)",
                updated + deleted,
                changed,
                deleted
            )
            .green()
        );
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Clear index database for current project
pub fn cmd_clear(root: &Path) -> Result<()> {
    db::delete_db(root)?;
    println!("Index cleared for {}", root.display());
    Ok(())
}

/// Show index statistics
pub fn cmd_stats(root: &Path) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index init' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let stats = db::get_stats(&conn)?;
    let db_path = db::get_db_path(root)?;
    let db_size = std::fs::metadata(&db_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // Detect project type
    let project_type = indexer::detect_project_type(root);

    println!("{}", "Index Statistics:".bold());
    println!("  Project:    {}", project_type.as_str());
    println!("  Files:      {}", stats.file_count);
    println!("  Symbols:    {}", stats.symbol_count);
    println!("  Refs:       {}", stats.refs_count);
    println!("  Modules:    {}", stats.module_count);

    // Show Android-specific stats if relevant
    if stats.xml_usages_count > 0 || stats.resources_count > 0 {
        println!("  XML usages: {}", stats.xml_usages_count);
        println!("  Resources:  {}", stats.resources_count);
    }

    // Show iOS-specific stats if relevant
    if stats.storyboard_usages_count > 0 || stats.ios_assets_count > 0 {
        println!("  Storyboard: {}", stats.storyboard_usages_count);
        println!("  iOS assets: {}", stats.ios_assets_count);
    }

    println!("  DB size:    {:.2} MB", db_size as f64 / 1024.0 / 1024.0);
    println!("  DB path:    {}", db_path.display());

    Ok(())
}
