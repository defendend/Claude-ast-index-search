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

/// File count threshold for auto-switching to sub-projects mode
const AUTO_SUB_PROJECTS_THRESHOLD: usize = 65_000;

/// Rebuild the index (full or partial)
pub fn cmd_rebuild(root: &Path, index_type: &str, index_deps: bool, no_ignore: bool, sub_projects: bool) -> Result<()> {
    // Explicit sub-projects mode
    if sub_projects {
        return cmd_rebuild_sub_projects(root, index_type, index_deps, no_ignore);
    }

    // Auto-detect: if sub-projects exist and file count >= threshold, switch automatically
    if index_type == "all" {
        let subs = indexer::find_sub_projects(root);
        if subs.len() >= 2 {
            let file_count = indexer::quick_file_count(root, no_ignore, AUTO_SUB_PROJECTS_THRESHOLD);
            if file_count >= AUTO_SUB_PROJECTS_THRESHOLD {
                eprintln!(
                    "{}",
                    format!(
                        "Detected {}+ files and {} sub-projects — switching to sub-projects mode automatically",
                        AUTO_SUB_PROJECTS_THRESHOLD, subs.len()
                    ).yellow()
                );
                return cmd_rebuild_sub_projects(root, index_type, index_deps, no_ignore);
            }
        }
    }

    let start = Instant::now();

    // Acquire exclusive lock to prevent concurrent rebuilds
    let _lock = db::acquire_rebuild_lock(root)?;

    // Save extra roots before deleting DB
    let saved_extra_roots = if db::db_exists(root) {
        let old_conn = db::open_db(root)?;
        db::get_extra_roots(&old_conn).unwrap_or_default()
    } else {
        vec![]
    };

    // Delete DB file entirely to avoid WAL hangs
    if let Err(e) = db::delete_db(root) {
        eprintln!("{}", format!("Warning: could not delete old index: {}", e).yellow());
        if let Ok(db_path) = db::get_db_path(root) {
            eprintln!("Cache path: {}", db_path.parent().unwrap_or(db_path.as_path()).display());
            eprintln!("Try manually removing the cache directory and re-running rebuild.");
        }
        return Err(e);
    }

    // Remove old kotlin-index cache dir entirely
    db::cleanup_legacy_cache();

    let mut conn = db::open_db(root)?;
    db::init_db(&conn)?;

    // Restore extra roots
    if !saved_extra_roots.is_empty() {
        let roots_json = serde_json::to_string(&saved_extra_roots)?;
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('extra_roots', ?1)",
            [&roots_json],
        )?;
    }

    // Store no_ignore setting in database metadata
    if no_ignore {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('no_ignore', '1')",
            [],
        ).ok();
        println!("{}", "Including gitignored files (build/, etc.)...".yellow());
    }

    // Detect project type — check actual platform markers for Mixed projects
    let project_type = indexer::detect_project_type(root);
    let is_ios = indexer::has_ios_markers(root);
    let is_android = indexer::has_android_markers(root);

    match index_type {
        "all" => {
            println!("{}", "Rebuilding full index...".cyan());
            let walk = indexer::index_directory(&mut conn, root, true, no_ignore)?;
            let mut file_count = walk.file_count;
            let module_count = indexer::index_modules_from_files(&conn, root, &walk.module_files)?;

            // Index extra roots
            let extra_roots = db::get_extra_roots(&conn)?;
            for extra_root in &extra_roots {
                let extra_path = std::path::Path::new(extra_root);
                if extra_path.exists() {
                    let extra_walk = indexer::index_directory(&mut conn, extra_path, true, no_ignore)?;
                    file_count += extra_walk.file_count;
                    println!("{}", format!("Indexed {} files from extra root: {}", extra_walk.file_count, extra_root).dimmed());
                }
            }

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
                dep_count = indexer::index_module_dependencies(&mut conn, root, &walk.module_files, true)?;
                trans_count = indexer::build_transitive_deps(&mut conn, true)?;
            }

            // Android-specific: XML layouts and resources
            let mut xml_count = 0;
            let mut res_count = 0;
            let mut res_usage_count = 0;
            if is_android {
                println!("{}", "Indexing XML layouts...".cyan());
                xml_count = indexer::index_xml_usages(&mut conn, root, &walk.xml_layout_files, true)?;

                println!("{}", "Indexing resources...".cyan());
                let (rc, ruc) = indexer::index_resources(&mut conn, root, &walk.res_files, true)?;
                res_count = rc;
                res_usage_count = ruc;
            }

            // iOS-specific: storyboards and assets
            let mut sb_count = 0;
            let mut asset_count = 0;
            let mut asset_usage_count = 0;
            if is_ios {
                println!("{}", "Indexing storyboards/xibs...".cyan());
                sb_count = indexer::index_storyboard_usages(&mut conn, root, &walk.storyboard_files, true)?;

                println!("{}", "Indexing iOS assets...".cyan());
                let (ac, auc) = indexer::index_ios_assets(&mut conn, root, &walk.xcassets_dirs, true)?;
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
            let walk = indexer::index_directory(&mut conn, root, true, no_ignore)?;
            println!("{}", format!("Indexed {} files", walk.file_count).green());
        }
        "modules" => {
            println!("{}", "Rebuilding modules index...".cyan());
            conn.execute("DELETE FROM module_deps", [])?;
            conn.execute("DELETE FROM modules", [])?;
            let module_count = indexer::index_modules(&conn, root)?;

            if index_deps {
                println!("{}", "Indexing module dependencies...".cyan());
                let gradle_files = indexer::collect_gradle_files_from_db(&conn, root)?;
                let dep_count = indexer::index_module_dependencies(&mut conn, root, &gradle_files, true)?;
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
            let gradle_files = indexer::collect_gradle_files_from_db(&conn, root)?;
            let dep_count = indexer::index_module_dependencies(&mut conn, root, &gradle_files, true)?;
            println!("{}", format!("Indexed {} dependencies", dep_count).green());
        }
        _ => {
            println!("{}", format!("Unknown index type: {}", index_type).red());
        }
    }

    eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    Ok(())
}

/// Rebuild index for each sub-project into a single shared DB for root
fn cmd_rebuild_sub_projects(root: &Path, _index_type: &str, _index_deps: bool, no_ignore: bool) -> Result<()> {
    let start = Instant::now();

    // Acquire exclusive lock to prevent concurrent rebuilds
    let _lock = db::acquire_rebuild_lock(root)?;

    let sub_projects = indexer::find_sub_projects(root);
    if sub_projects.is_empty() {
        println!("{}", "No sub-projects found. Use 'rebuild' without --sub-projects.".yellow());
        return Ok(());
    }

    let total = sub_projects.len();
    println!(
        "{}",
        format!("Found {} sub-projects in {}:", total, root.display()).cyan()
    );
    for (path, pt) in &sub_projects {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        println!("  {} [{}]", name, pt.as_str());
    }
    println!();

    // Single DB for the whole root
    if let Err(e) = db::delete_db(root) {
        eprintln!("{}", format!("Warning: could not delete old index: {}", e).yellow());
        return Err(e);
    }
    let mut conn = db::open_db(root)?;
    db::init_db(&conn)?;

    if no_ignore {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('no_ignore', '1')",
            [],
        ).ok();
    }

    let mut total_files = 0;
    let mut success_count = 0;
    let mut fail_count = 0;

    for (i, (path, pt)) in sub_projects.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        println!(
            "{}",
            format!("[{}/{}] Indexing {} [{}]...", i + 1, total, name, pt.as_str()).cyan()
        );

        match indexer::index_directory_scoped(&mut conn, root, path, true, no_ignore) {
            Ok(walk) => {
                total_files += walk.file_count;
                println!(
                    "{}",
                    format!("  {} files indexed", walk.file_count).dimmed()
                );
                success_count += 1;
            }
            Err(e) => {
                println!("{}", format!("  Failed: {}", e).red());
                fail_count += 1;
            }
        }
    }

    println!();
    println!(
        "{}",
        format!(
            "Done: {} sub-projects indexed ({} files total), {} failed",
            success_count, total_files, fail_count
        ).green()
    );
    eprintln!("{}", format!("Total time: {:?}", start.elapsed()).dimmed());
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

/// Restore index from a .db file
pub fn cmd_restore(root: &Path, db_file: &str) -> Result<()> {
    let src = std::path::Path::new(db_file);

    if !src.exists() {
        anyhow::bail!("File not found: {}", db_file);
    }
    if !src.is_file() {
        anyhow::bail!("Not a file: {}", db_file);
    }

    let dest = db::get_db_path(root)?;
    let dest_dir = dest.parent().unwrap();
    std::fs::create_dir_all(dest_dir)?;

    // Remove existing DB files if present
    if db::db_exists(root) {
        db::delete_db(root)?;
    }

    std::fs::copy(src, &dest)?;

    // Copy WAL/SHM if they exist alongside the source
    for suffix in ["-wal", "-shm"] {
        let src_extra = src.with_extension(format!("db{}", suffix));
        if src_extra.exists() {
            let dest_extra = dest.with_extension(format!("db{}", suffix));
            std::fs::copy(&src_extra, &dest_extra)?;
        }
    }

    // Update project_root metadata to match current project
    let conn = db::open_db(root)?;
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('project_root', ?1)",
        [root.to_string_lossy().as_ref()],
    )?;

    println!("{}", format!("Restored index from: {}", db_file).green());
    println!("DB path: {}", dest.display());

    // Show quick stats
    let stats = db::get_stats(&conn)?;
    println!(
        "{}",
        format!(
            "Contains: {} files, {} symbols, {} refs",
            stats.file_count, stats.symbol_count, stats.refs_count
        ).dimmed()
    );

    Ok(())
}

/// Clear index database for current project
pub fn cmd_clear(root: &Path) -> Result<()> {
    db::delete_db(root)?;
    println!("Index cleared for {}", root.display());
    Ok(())
}

/// Show index statistics
pub fn cmd_stats(root: &Path, format: &str) -> Result<()> {
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

    if format == "json" {
        let result = serde_json::json!({
            "project": indexer::detect_project_type(root).as_str(),
            "stats": stats,
            "db_size_bytes": db_size,
            "db_path": db_path.display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

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

    // Show extra roots if any
    let extra_roots = db::get_extra_roots(&conn)?;
    if !extra_roots.is_empty() {
        println!("\n  Extra roots:");
        for r in &extra_roots {
            println!("    {}", r);
        }
    }

    Ok(())
}

/// Add an extra source root
pub fn cmd_add_root(root: &Path, path: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let abs_path = if std::path::Path::new(path).is_absolute() {
        path.to_string()
    } else {
        let cwd = std::env::current_dir()?;
        cwd.join(path).to_string_lossy().to_string()
    };

    let conn = db::open_db(root)?;
    db::add_extra_root(&conn, &abs_path)?;
    println!("{}", format!("Added source root: {}", abs_path).green());
    Ok(())
}

/// Remove an extra source root
pub fn cmd_remove_root(root: &Path, path: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let abs_path = if std::path::Path::new(path).is_absolute() {
        path.to_string()
    } else {
        let cwd = std::env::current_dir()?;
        cwd.join(path).to_string_lossy().to_string()
    };

    let conn = db::open_db(root)?;
    if db::remove_extra_root(&conn, &abs_path)? {
        println!("{}", format!("Removed source root: {}", abs_path).green());
    } else {
        println!("{}", format!("Root not found: {}", abs_path).yellow());
    }
    Ok(())
}

/// List configured source roots
pub fn cmd_list_roots(root: &Path) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let extra_roots = db::get_extra_roots(&conn)?;

    println!("{}", "Source roots:".bold());
    println!("  {} (primary)", root.display());
    for r in &extra_roots {
        println!("  {}", r);
    }

    Ok(())
}
