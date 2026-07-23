//! Index management commands
//!
//! Commands for managing the code index:
//! - rebuild: Rebuild the index (full or partial)
//! - update: Incrementally update the index
//! - stats: Show index statistics

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::db;
use crate::indexer;

/// File count threshold for auto-switching to sub-projects mode
const AUTO_SUB_PROJECTS_THRESHOLD: usize = 65_000;
/// A root with this many sub-projects is treated as a monorepo immediately and
/// skips the expensive quick file count.
const SUB_PROJECTS_SHORTCUT_THRESHOLD: usize = 20;

pub(crate) struct ScopedEnvVar {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl ScopedEnvVar {
    pub(crate) fn set_bool(key: &'static str, enabled: bool) -> Self {
        let previous = std::env::var_os(key);
        if enabled {
            std::env::set_var(key, "1");
        } else {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(prev) = self.previous.take() {
            std::env::set_var(self.key, prev);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn init_rebuild_schema(conn: &rusqlite::Connection) -> Result<()> {
    db::enable_rebuild_pragmas(conn)?;
    db::init_db_for_rebuild(conn)
}

fn finalize_rebuild_schema(conn: &rusqlite::Connection, verbose: bool) -> Result<()> {
    let t = Instant::now();
    db::finalize_db_after_rebuild(conn)?;
    if verbose {
        eprintln!("[verbose] finalize_db_after_rebuild in {:?}", t.elapsed());
    }
    Ok(())
}

fn restore_rebuild_pragmas(conn: &rusqlite::Connection, verbose: bool) -> Result<()> {
    let t = Instant::now();
    db::restore_rebuild_pragmas(conn)?;
    if verbose {
        eprintln!("[verbose] restore_rebuild_pragmas in {:?}", t.elapsed());
    }
    Ok(())
}

/// Build a gitignore-style exclude matcher anchored to `root` from config patterns.
fn build_exclude_matcher(
    root: &std::path::Path,
    patterns: Option<&[String]>,
) -> Option<ignore::gitignore::Gitignore> {
    let patterns = patterns?;
    if patterns.is_empty() {
        return None;
    }
    let mut gb = ignore::gitignore::GitignoreBuilder::new(root);
    for p in patterns {
        gb.add_line(None, p).ok();
    }
    gb.build().ok()
}

fn snapshot_subtrees(root: &Path, verbose: bool) -> Result<Vec<db::Subtree>> {
    if !db::db_exists(root) {
        return Ok(Vec::new());
    }
    if verbose {
        eprintln!("[verbose] reading subtrees from existing DB...");
    }
    let old_conn = db::open_db_leased(root)?;
    // `open_db` eagerly and transactionally migrates pre-3.47 extra_roots.
    db::list_subtrees(&old_conn)
}

fn attach_rebuild_subtrees(
    conn: &rusqlite::Connection,
    root: &Path,
    saved_subtrees: &[db::Subtree],
    config_roots: Option<&[String]>,
    extra_paths: &[String],
    verbose: bool,
) -> Result<()> {
    let mut taken_canonicals: HashSet<String> = HashSet::new();
    for sub in saved_subtrees {
        db::insert_subtree(conn, &sub.name, &sub.canonical_path, &sub.original_path)?;
        taken_canonicals.insert(sub.canonical_path.clone());
    }

    // Append config-supplied roots and CLI `--path` args. Each new path
    // gets an auto-allocated subtree name (basename, with `-N` suffix on
    // collision). Paths that already match an existing subtree by their
    // canonical form are deduplicated.
    let mut attach_extra = |raw: &str, label: &str| -> Result<()> {
        let candidate = if std::path::Path::new(raw).is_absolute() {
            std::path::PathBuf::from(raw)
        } else {
            root.join(raw)
        };
        let canonical = db::safe_canonicalize(&candidate)
            .to_string_lossy()
            .into_owned();
        if taken_canonicals.contains(&canonical) {
            return Ok(());
        }
        let preferred = db::default_subtree_name(&canonical);
        let name = db::allocate_subtree_name(conn, &preferred)?;
        db::insert_subtree(conn, &name, &canonical, raw)?;
        taken_canonicals.insert(canonical);
        if verbose {
            eprintln!("[verbose] attached {}: {}", label, raw);
        }
        Ok(())
    };

    if let Some(config_roots) = config_roots {
        for cr in config_roots {
            attach_extra(cr, "config root")?;
        }
    }
    for p in extra_paths {
        attach_extra(p, "--path")?;
    }
    Ok(())
}

/// Rebuild the index (full or partial)
pub fn cmd_rebuild(
    root: &Path,
    index_type: &str,
    index_deps: bool,
    no_ignore: bool,
    sub_projects: bool,
    verbose: bool,
    experimental_fast_rebuild: bool,
    cli_include: &[String],
    cli_exclude: &[String],
    extra_paths: &[String],
) -> Result<()> {
    let _experimental_fast_rebuild_env = ScopedEnvVar::set_bool(
        "AST_INDEX_EXPERIMENTAL_FAST_REBUILD",
        experimental_fast_rebuild,
    );
    // Rebuild performs potentially long config and sub-project discovery
    // before acquiring its exclusive rebuild lock. Keep the previous index
    // leased from the first cache access so GC cannot remove it before the
    // subtree snapshot/swap sequence begins.
    let _cache_lease = db::acquire_project_lease(root)?;
    if verbose {
        std::env::set_var("AST_INDEX_VERBOSE", "1");
        eprintln!("[verbose] rebuild started for: {}", root.display());
        eprintln!(
            "[verbose] index_type={}, index_deps={}, no_ignore={}, sub_projects={}",
            index_type, index_deps, no_ignore, sub_projects
        );
        // Each line below is a separate verbose checkpoint so that a future
        // "hangs after X" report points to the exact failing syscall.
        eprintln!("[verbose] resolving db path...");
    }
    let db_path_lookup = db::get_db_path(root).ok();
    if verbose {
        eprintln!("[verbose] db path: {:?}", db_path_lookup);
        eprintln!("[verbose] loading .ast-index.yaml...");
    }

    // Load project config (.ast-index.yaml)
    let config = indexer::load_config(root).unwrap_or_default();
    if verbose {
        eprintln!("[verbose] config: {:?}", config);
    }

    // Apply config fallbacks: CLI flags > config > defaults
    let no_ignore = if no_ignore {
        true
    } else {
        config.no_ignore.unwrap_or(false)
    };
    // Merge CLI flags with config: CLI overrides config
    let mut merged_exclude: Vec<String> = config.exclude.unwrap_or_default();
    for e in cli_exclude {
        if !merged_exclude.contains(e) {
            merged_exclude.push(e.clone());
        }
    }
    let config_exclude: Option<Vec<String>> = if merged_exclude.is_empty() {
        None
    } else {
        Some(merged_exclude)
    };

    let mut merged_include: Vec<String> = config.include.unwrap_or_default();
    for i in cli_include {
        if !merged_include.contains(i) {
            merged_include.push(i.clone());
        }
    }
    let config_include: Option<Vec<String>> = if merged_include.is_empty() {
        None
    } else {
        Some(merged_include)
    };

    let config_roots = config.roots.clone();

    // Build exclude matcher once — reused for sub-project filtering and directory walks
    let exclude_matcher = build_exclude_matcher(root, config_exclude.as_deref());

    if verbose {
        if let Some(ref inc) = config_include {
            eprintln!("[verbose] include (allow-list): {:?}", inc);
        }
        if let Some(ref exc) = config_exclude {
            eprintln!("[verbose] exclude: {} patterns", exc.len());
        }
    }

    // Explicit sub-projects mode (--sub-projects flag)
    if sub_projects {
        return cmd_rebuild_sub_projects(
            root,
            index_type,
            index_deps,
            no_ignore,
            verbose,
            experimental_fast_rebuild,
            config_exclude.as_deref(),
            config_include.as_deref(),
            config_roots.as_deref(),
            extra_paths,
            exclude_matcher.as_ref(),
        );
    }

    // Auto-detect: scan immediate subdirs (with exclude+include filter) and check file count
    if index_type == "all" {
        let t = Instant::now();
        let subs =
            indexer::find_sub_projects(root, exclude_matcher.as_ref(), config_include.as_deref());
        if verbose {
            eprintln!(
                "[verbose] find_sub_projects: {} found in {:?}",
                subs.len(),
                t.elapsed()
            );
        }

        // When `include` is set explicitly, always honor it — route through the scoped
        // path so the walker only touches the listed directories (not the whole root).
        // Without this, a small project that sets include would fall through to the main
        // branch and the include filter would be silently ignored.
        if config_include.is_some() && !subs.is_empty() {
            eprintln!(
                "{}",
                format!(
                    "Honoring include config ({} paths) — walking only listed directories",
                    subs.len()
                )
                .yellow()
            );
            return cmd_rebuild_sub_projects(
                root,
                index_type,
                index_deps,
                no_ignore,
                verbose,
                experimental_fast_rebuild,
                config_exclude.as_deref(),
                config_include.as_deref(),
                config_roots.as_deref(),
                extra_paths,
                exclude_matcher.as_ref(),
            );
        }

        if subs.len() >= 2 {
            if subs.len() >= SUB_PROJECTS_SHORTCUT_THRESHOLD {
                eprintln!(
                    "{}",
                    format!(
                        "Detected {} sub-projects — skipping quick file count and switching to sub-projects mode",
                        subs.len()
                    ).yellow()
                );
                return cmd_rebuild_sub_projects(
                    root,
                    index_type,
                    index_deps,
                    no_ignore,
                    verbose,
                    experimental_fast_rebuild,
                    config_exclude.as_deref(),
                    config_include.as_deref(),
                    config_roots.as_deref(),
                    extra_paths,
                    exclude_matcher.as_ref(),
                );
            }
            if verbose {
                eprintln!(
                    "[verbose] counting files (quick_file_count, limit={})...",
                    AUTO_SUB_PROJECTS_THRESHOLD
                );
            }
            let t = Instant::now();
            let file_count =
                indexer::quick_file_count(root, no_ignore, AUTO_SUB_PROJECTS_THRESHOLD);
            if verbose {
                eprintln!(
                    "[verbose] quick_file_count: {} in {:?}",
                    file_count,
                    t.elapsed()
                );
            }
            if file_count >= AUTO_SUB_PROJECTS_THRESHOLD {
                eprintln!(
                    "{}",
                    format!(
                        "Detected {}+ files and {} sub-projects — switching to sub-projects mode automatically",
                        AUTO_SUB_PROJECTS_THRESHOLD, subs.len()
                    ).yellow()
                );
                return cmd_rebuild_sub_projects(
                    root,
                    index_type,
                    index_deps,
                    no_ignore,
                    verbose,
                    experimental_fast_rebuild,
                    config_exclude.as_deref(),
                    config_include.as_deref(),
                    config_roots.as_deref(),
                    extra_paths,
                    exclude_matcher.as_ref(),
                );
            }
        }
    }

    let start = Instant::now();

    // Acquire exclusive lock to prevent concurrent rebuilds
    if verbose {
        eprintln!("[verbose] acquiring rebuild lock...");
    }
    let t = Instant::now();
    let _lock = db::acquire_rebuild_guard(root)?;
    if verbose {
        eprintln!("[verbose] lock acquired in {:?}", t.elapsed());
    }
    db::recover_interrupted_index_publication(root)?;

    // Save subtree triples (name + canonical_path + original_path) before
    // building the replacement. Going through `get_extra_roots` (legacy
    // shim) would collapse each row to a bare canonical path string and
    // lose the user-chosen name + original (relative) path form on every
    // rebuild — re-importing them as `<basename>` and `original=canonical`.
    let saved_subtrees = snapshot_subtrees(root, verbose)?;

    // Build in a private generation beside the live DB. Readers retain the
    // previous complete generation until the short publication handoff.
    if verbose {
        eprintln!("[verbose] allocating staged DB generation...");
    }
    let t = Instant::now();
    let live_db = db::get_db_path(root)?;
    let staged = IndexStaging::create(&live_db, "rebuild")?;
    if verbose {
        eprintln!("[verbose] staged generation allocated in {:?}", t.elapsed());
    }

    // Remove old kotlin-index cache dir entirely
    db::cleanup_legacy_cache();

    if verbose {
        eprintln!("[verbose] opening new DB...");
    }
    let t = Instant::now();
    let mut conn = db::open_staged_db(root, staged.db_path())?;
    init_rebuild_schema(&conn)?;
    if verbose {
        eprintln!(
            "[verbose] staged DB opened + schema created in {:?}",
            t.elapsed()
        );
    }

    // Reattach saved subtrees verbatim so user-chosen names and the
    // original path form (relative vs absolute) survive the rebuild.
    attach_rebuild_subtrees(
        &conn,
        root,
        &saved_subtrees,
        config_roots.as_deref(),
        extra_paths,
        verbose,
    )?;

    // Store no_ignore setting in database metadata
    if no_ignore {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('no_ignore', '1')",
            [],
        )
        .ok();
        println!(
            "{}",
            "Including gitignored files (build/, etc.)...".yellow()
        );
    }
    // Persist the cap bypass when the user passed `--force --remember`.
    // Set early so the walker (called below) already sees `bypass_size_check`
    // in metadata and doesn't trip the cap on this very run.
    if std::env::var("AST_INDEX_REMEMBER_BYPASS").is_ok() {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('bypass_size_check', '1')",
            [],
        )
        .ok();
        println!(
            "{}",
            "Persisted --force opt-in for this project. Future `rebuild` runs \
             will not hit the candidate-file cap on this root."
                .green()
        );
    }
    db::set_experimental_fast_rebuild_enabled(&conn, experimental_fast_rebuild).ok();

    // Check actual platform markers for mixed mobile repos.
    //
    // Marker check covers the canonical case (build.gradle / Package.swift
    // at the root). Both flags are promoted below from walker output as well
    // — a monorepo where the marker lives one level deeper still gets its
    // resources / storyboards indexed.
    let mut is_ios = indexer::has_ios_markers(root);
    let mut is_android = indexer::has_android_markers(root);

    match index_type {
        "all" => {
            println!("{}", "Rebuilding full index...".cyan());
            if verbose {
                eprintln!("[verbose] starting file walk + parse...");
            }
            let t = Instant::now();
            let walk = indexer::index_directory_with_config(
                &mut conn,
                root,
                true,
                no_ignore,
                config_exclude.as_deref(),
            )?;
            let mut file_count = walk.file_count;
            if verbose {
                eprintln!(
                    "[verbose] index_directory: {} files in {:?}",
                    file_count,
                    t.elapsed()
                );
            }

            // Promote platform flags from collected artefacts — see comment at
            // the marker-based detection above.
            if !walk.res_files.is_empty() {
                is_android = true;
            }
            if !walk.storyboard_files.is_empty() || !walk.xcassets_dirs.is_empty() {
                is_ios = true;
            }

            // Collect module_files from primary root
            let mut all_module_files = walk.module_files;

            // Index extra roots and merge their module_files
            let extra_roots = db::get_extra_roots(&conn)?;
            for extra_root in &extra_roots {
                let extra_path = std::path::Path::new(extra_root);
                if extra_path.exists() {
                    if verbose {
                        eprintln!("[verbose] indexing extra root: {}", extra_root);
                    }
                    let t = Instant::now();
                    let extra_walk = indexer::index_directory_with_config(
                        &mut conn,
                        extra_path,
                        true,
                        no_ignore,
                        config_exclude.as_deref(),
                    )?;
                    file_count += extra_walk.file_count;
                    all_module_files.extend(extra_walk.module_files);
                    if verbose {
                        eprintln!(
                            "[verbose] extra root: {} files in {:?}",
                            extra_walk.file_count,
                            t.elapsed()
                        );
                    }
                    println!(
                        "{}",
                        format!(
                            "Indexed {} files from extra root: {}",
                            extra_walk.file_count, extra_root
                        )
                        .dimmed()
                    );
                }
            }

            let t = Instant::now();
            let module_count = indexer::index_modules_from_files(&conn, root, &all_module_files)?;
            if verbose {
                eprintln!(
                    "[verbose] index_modules: {} modules in {:?}",
                    module_count,
                    t.elapsed()
                );
            }

            // Index CocoaPods/Carthage for iOS
            if is_ios {
                if verbose {
                    eprintln!("[verbose] indexing CocoaPods/Carthage...");
                }
                let t = Instant::now();
                let pkg_count = indexer::index_ios_package_managers(&conn, root, true)?;
                if verbose {
                    eprintln!(
                        "[verbose] ios_package_managers: {} in {:?}",
                        pkg_count,
                        t.elapsed()
                    );
                }
                if pkg_count > 0 {
                    println!(
                        "{}",
                        format!("Indexed {} CocoaPods/Carthage deps", pkg_count).dimmed()
                    );
                }
            }

            let mut dep_count = 0;
            let mut trans_count = 0;
            // Run dep indexing whenever there are modules to process. This covers
            // Android/Gradle, Maven, ya.make, and Python projects — previously this
            // step was gated on Android detection, silently skipping other build systems.
            if index_deps && module_count > 0 {
                println!("{}", "Indexing module dependencies...".cyan());
                if verbose {
                    eprintln!("[verbose] indexing module deps...");
                }
                let t = Instant::now();
                dep_count =
                    indexer::index_module_dependencies(&mut conn, root, &all_module_files, true)?;
                if verbose {
                    eprintln!("[verbose] module_deps: {} in {:?}", dep_count, t.elapsed());
                }
                let t = Instant::now();
                trans_count = indexer::build_transitive_deps(&mut conn, true)?;
                if verbose {
                    eprintln!(
                        "[verbose] transitive_deps: {} in {:?}",
                        trans_count,
                        t.elapsed()
                    );
                }
            }

            // Frontend-specific: .d.ts from node_modules
            let mut dts_count = 0;
            if root.join("node_modules").exists() {
                if verbose {
                    eprintln!("[verbose] indexing .d.ts from node_modules...");
                }
                let t = Instant::now();
                dts_count = indexer::index_node_modules_dts(&mut conn, root, true)?;
                if verbose {
                    eprintln!(
                        "[verbose] node_modules .d.ts: {} files in {:?}",
                        dts_count,
                        t.elapsed()
                    );
                }
                if dts_count > 0 {
                    println!(
                        "{}",
                        format!(
                            "Indexed {} .d.ts type declarations from node_modules",
                            dts_count
                        )
                        .dimmed()
                    );
                }
            }

            // Android-specific: XML layouts and resources
            let mut xml_count = 0;
            let mut res_count = 0;
            let mut res_usage_count = 0;
            if is_android {
                println!("{}", "Indexing XML layouts...".cyan());
                let t = Instant::now();
                xml_count =
                    indexer::index_xml_usages(&mut conn, root, &walk.xml_layout_files, true)?;
                if verbose {
                    eprintln!("[verbose] xml_usages: {} in {:?}", xml_count, t.elapsed());
                }

                println!("{}", "Indexing resources...".cyan());
                let t = Instant::now();
                let (rc, ruc) = indexer::index_resources(&mut conn, root, &walk.res_files, true)?;
                res_count = rc;
                res_usage_count = ruc;
                if verbose {
                    eprintln!(
                        "[verbose] resources: {} defs, {} usages in {:?}",
                        res_count,
                        res_usage_count,
                        t.elapsed()
                    );
                }
            }

            // iOS-specific: storyboards and assets
            let mut sb_count = 0;
            let mut asset_count = 0;
            let mut asset_usage_count = 0;
            if is_ios {
                println!("{}", "Indexing storyboards/xibs...".cyan());
                let t = Instant::now();
                sb_count = indexer::index_storyboard_usages(
                    &mut conn,
                    root,
                    &walk.storyboard_files,
                    true,
                )?;
                if verbose {
                    eprintln!(
                        "[verbose] storyboard_usages: {} in {:?}",
                        sb_count,
                        t.elapsed()
                    );
                }

                println!("{}", "Indexing iOS assets...".cyan());
                let t = Instant::now();
                let (ac, auc) =
                    indexer::index_ios_assets(&mut conn, root, &walk.xcassets_dirs, true)?;
                asset_count = ac;
                asset_usage_count = auc;
                if verbose {
                    eprintln!(
                        "[verbose] ios_assets: {} defs, {} usages in {:?}",
                        asset_count,
                        asset_usage_count,
                        t.elapsed()
                    );
                }
            }

            // Print summary based on which platform-specific indexes ran.
            finalize_rebuild_schema(&conn, verbose)?;

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
                    )
                    .green()
                );
            } else if dts_count > 0 {
                println!(
                    "{}",
                    format!(
                        "Indexed {} files (+{} .d.ts), {} modules, {} deps",
                        file_count, dts_count, module_count, dep_count
                    )
                    .green()
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
            let walk = indexer::index_directory_with_config(
                &mut conn,
                root,
                true,
                no_ignore,
                config_exclude.as_deref(),
            )?;
            finalize_rebuild_schema(&conn, verbose)?;
            println!("{}", format!("Indexed {} files", walk.file_count).green());
        }
        "modules" => {
            println!("{}", "Rebuilding modules index...".cyan());
            conn.execute("DELETE FROM module_deps", [])?;
            conn.execute("DELETE FROM modules", [])?;
            let module_count = indexer::index_modules(&conn, root)?;

            if index_deps {
                println!("{}", "Indexing module dependencies...".cyan());
                let gradle_files = indexer::collect_build_files_from_db(&conn, root)?;
                let dep_count =
                    indexer::index_module_dependencies(&mut conn, root, &gradle_files, true)?;
                finalize_rebuild_schema(&conn, verbose)?;
                println!(
                    "{}",
                    format!(
                        "Indexed {} modules, {} dependencies",
                        module_count, dep_count
                    )
                    .green()
                );
            } else {
                finalize_rebuild_schema(&conn, verbose)?;
                println!("{}", format!("Indexed {} modules", module_count).green());
            }
        }
        "deps" => {
            println!("{}", "Indexing module dependencies...".cyan());
            let gradle_files = indexer::collect_build_files_from_db(&conn, root)?;
            let dep_count =
                indexer::index_module_dependencies(&mut conn, root, &gradle_files, true)?;
            finalize_rebuild_schema(&conn, verbose)?;
            println!("{}", format!("Indexed {} dependencies", dep_count).green());
        }
        _ => {
            println!("{}", format!("Unknown index type: {}", index_type).red());
        }
    }

    match index_type {
        "all" => {
            db::mark_index_updated(&conn)?;
            db::mark_modules_indexed(&conn)?;
        }
        "files" | "symbols" => db::mark_index_updated(&conn)?,
        "modules" | "deps" => db::mark_modules_indexed(&conn)?,
        _ => {}
    }

    if verbose {
        eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    }
    restore_rebuild_pragmas(&conn, verbose)?;
    db::seal_staged_db(conn, staged.db_path())?;
    let publication = db::acquire_index_publication_guard(root)?;
    publication.install_staged(staged.db_path())?;
    Ok(())
}

/// Rebuild index for each sub-project into a single shared DB for root.
/// `config_include` — allow-list of directories (relative to root); when set, only matching dirs are indexed.
/// `exclude_matcher` — gitignore-style matcher for filtering sub-projects.
fn cmd_rebuild_sub_projects(
    root: &Path,
    _index_type: &str,
    _index_deps: bool,
    no_ignore: bool,
    verbose: bool,
    experimental_fast_rebuild: bool,
    extra_exclude: Option<&[String]>,
    config_include: Option<&[String]>,
    config_roots: Option<&[String]>,
    extra_paths: &[String],
    exclude_matcher: Option<&ignore::gitignore::Gitignore>,
) -> Result<()> {
    let start = Instant::now();

    // Acquire exclusive lock to prevent concurrent rebuilds
    if verbose {
        eprintln!("[verbose] sub-projects: acquiring lock...");
    }
    let t = Instant::now();
    let _lock = db::acquire_rebuild_guard(root)?;
    if verbose {
        eprintln!("[verbose] lock acquired in {:?}", t.elapsed());
    }
    db::recover_interrupted_index_publication(root)?;

    let t = Instant::now();
    let sub_projects = indexer::find_sub_projects(root, exclude_matcher, config_include);
    if verbose {
        eprintln!(
            "[verbose] find_sub_projects: {} in {:?}",
            sub_projects.len(),
            t.elapsed()
        );
    }
    if sub_projects.is_empty() {
        println!(
            "{}",
            "No sub-projects found. Use 'rebuild' without --sub-projects.".yellow()
        );
        return Ok(());
    }

    let total = sub_projects.len();
    println!(
        "{}",
        format!("Found {} sub-projects in {}:", total, root.display()).cyan()
    );
    for (path, _) in &sub_projects {
        let name = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
        println!("  {}", name);
    }
    println!();

    let saved_subtrees = snapshot_subtrees(root, verbose)?;

    // Build beside the live generation — see cmd_rebuild for rationale.
    if verbose {
        eprintln!("[verbose] allocating staged DB generation...");
    }
    let t = Instant::now();
    let live_db = db::get_db_path(root)?;
    let staged = IndexStaging::create(&live_db, "rebuild")?;
    let mut conn = db::open_staged_db(root, staged.db_path())?;
    init_rebuild_schema(&conn)?;
    if verbose {
        eprintln!("[verbose] staged DB opened in {:?}", t.elapsed());
    }

    attach_rebuild_subtrees(
        &conn,
        root,
        &saved_subtrees,
        config_roots,
        extra_paths,
        verbose,
    )?;

    if no_ignore {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('no_ignore', '1')",
            [],
        )
        .ok();
    }
    db::set_experimental_fast_rebuild_enabled(&conn, experimental_fast_rebuild).ok();

    let mut total_files = 0;
    let mut success_count = 0;
    let mut fail_count = 0;
    let mut all_module_files = Vec::new();
    let mut all_xml_files = Vec::new();
    let mut all_res_files = Vec::new();
    let mut all_storyboard_files = Vec::new();
    let mut all_xcassets_dirs = Vec::new();
    let mut any_android = false;
    let mut any_ios = false;

    if config_include.is_none() {
        let t = Instant::now();
        match indexer::index_directory_direct_entries(
            &mut conn,
            root,
            false,
            no_ignore,
            extra_exclude,
        ) {
            Ok(walk) => {
                total_files += walk.file_count;
                all_module_files.extend(walk.module_files);
                all_xml_files.extend(walk.xml_layout_files);
                all_res_files.extend(walk.res_files);
                all_storyboard_files.extend(walk.storyboard_files);
                all_xcassets_dirs.extend(walk.xcassets_dirs);
                if verbose {
                    eprintln!(
                        "[verbose] root direct entries: {} files in {:?}",
                        walk.file_count,
                        t.elapsed()
                    );
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!(
                        "[verbose] root direct entries: FAILED in {:?}: {}",
                        t.elapsed(),
                        e
                    );
                }
                println!("{}", format!("  Root direct entries failed: {}", e).red());
                fail_count += 1;
            }
        }
    }

    for (i, (path, _)) in sub_projects.iter().enumerate() {
        let name = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
        println!(
            "{}",
            format!("[{}/{}] Indexing {}...", i + 1, total, name).cyan()
        );

        if indexer::has_android_markers(path) {
            any_android = true;
        }
        if indexer::has_ios_markers(path) {
            any_ios = true;
        }

        let t = Instant::now();
        match indexer::index_directory_scoped(&mut conn, root, path, true, no_ignore, extra_exclude)
        {
            Ok(walk) => {
                total_files += walk.file_count;
                // Promote Android/iOS flags from collected artefacts as well —
                // a sub-project may carry res/storyboard files even when the
                // canonical marker file (build.gradle, Package.swift) lives one
                // level deeper and `has_*_markers(path)` returned false.
                if !walk.res_files.is_empty() {
                    any_android = true;
                }
                if !walk.storyboard_files.is_empty() || !walk.xcassets_dirs.is_empty() {
                    any_ios = true;
                }
                all_module_files.extend(walk.module_files);
                all_xml_files.extend(walk.xml_layout_files);
                all_res_files.extend(walk.res_files);
                all_storyboard_files.extend(walk.storyboard_files);
                all_xcassets_dirs.extend(walk.xcassets_dirs);
                if verbose {
                    eprintln!(
                        "[verbose] {} — {} files in {:?}",
                        name,
                        walk.file_count,
                        t.elapsed()
                    );
                }
                println!(
                    "{}",
                    format!("  {} files indexed", walk.file_count).dimmed()
                );
                success_count += 1;
            }
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] {} — FAILED in {:?}: {}", name, t.elapsed(), e);
                }
                println!("{}", format!("  Failed: {}", e).red());
                fail_count += 1;
            }
        }
    }

    let extra_roots = db::get_extra_roots(&conn)?;
    for extra_root in &extra_roots {
        let extra_path = std::path::Path::new(extra_root);
        if !extra_path.exists() {
            continue;
        }
        if verbose {
            eprintln!("[verbose] indexing extra root: {}", extra_root);
        }
        let t = Instant::now();
        let extra_walk = indexer::index_directory_with_config(
            &mut conn,
            extra_path,
            true,
            no_ignore,
            extra_exclude,
        )?;
        total_files += extra_walk.file_count;
        if !extra_walk.res_files.is_empty() {
            any_android = true;
        }
        if !extra_walk.storyboard_files.is_empty() || !extra_walk.xcassets_dirs.is_empty() {
            any_ios = true;
        }
        all_module_files.extend(extra_walk.module_files);
        all_xml_files.extend(extra_walk.xml_layout_files);
        all_res_files.extend(extra_walk.res_files);
        all_storyboard_files.extend(extra_walk.storyboard_files);
        all_xcassets_dirs.extend(extra_walk.xcassets_dirs);
        if verbose {
            eprintln!(
                "[verbose] extra root: {} files in {:?}",
                extra_walk.file_count,
                t.elapsed()
            );
        }
        println!(
            "{}",
            format!(
                "Indexed {} files from extra root: {}",
                extra_walk.file_count, extra_root
            )
            .dimmed()
        );
    }

    // Index modules and dependencies from collected build files
    let t = Instant::now();
    let module_count = indexer::index_modules_from_files(&conn, root, &all_module_files)?;
    if verbose {
        eprintln!(
            "[verbose] index_modules: {} modules in {:?}",
            module_count,
            t.elapsed()
        );
    }

    let mut dep_count = 0;
    let mut trans_count = 0;
    if module_count > 0 {
        let t = Instant::now();
        dep_count =
            indexer::index_module_dependencies(&mut conn, root, &all_module_files, verbose)?;
        if verbose {
            eprintln!("[verbose] module_deps: {} in {:?}", dep_count, t.elapsed());
        }
        let t = Instant::now();
        trans_count = indexer::build_transitive_deps(&mut conn, verbose)?;
        if verbose {
            eprintln!(
                "[verbose] transitive_deps: {} in {:?}",
                trans_count,
                t.elapsed()
            );
        }
    }

    // Android-specific: XML layouts and resources.
    // Split: a sub-project with values/drawables/colors but no layout/menu/
    // navigation XML still needs resources indexed.
    if any_android {
        if !all_xml_files.is_empty() {
            let t = Instant::now();
            let xml_count = indexer::index_xml_usages(&mut conn, root, &all_xml_files, verbose)?;
            if verbose {
                eprintln!("[verbose] xml_usages: {} in {:?}", xml_count, t.elapsed());
            }
        }
        if !all_res_files.is_empty() {
            let t = Instant::now();
            let (res_count, _) =
                indexer::index_resources(&mut conn, root, &all_res_files, verbose)?;
            if verbose {
                eprintln!("[verbose] resources: {} in {:?}", res_count, t.elapsed());
            }
        }
    }

    // iOS-specific: storyboards and assets
    if any_ios {
        if !all_storyboard_files.is_empty() {
            let t = Instant::now();
            let sb_count =
                indexer::index_storyboard_usages(&mut conn, root, &all_storyboard_files, verbose)?;
            if verbose {
                eprintln!(
                    "[verbose] storyboard_usages: {} in {:?}",
                    sb_count,
                    t.elapsed()
                );
            }
        }
        if !all_xcassets_dirs.is_empty() {
            let t = Instant::now();
            let (asset_count, _) =
                indexer::index_ios_assets(&mut conn, root, &all_xcassets_dirs, verbose)?;
            if verbose {
                eprintln!("[verbose] ios_assets: {} in {:?}", asset_count, t.elapsed());
            }
        }
    }

    finalize_rebuild_schema(&conn, verbose)?;
    db::mark_index_updated(&conn)?;
    db::mark_modules_indexed(&conn)?;

    println!();
    println!(
        "{}",
        format!(
            "Done: {} sub-projects indexed ({} files, {} modules, {} deps, {} transitive), {} failed",
            success_count, total_files, module_count, dep_count, trans_count, fail_count
        ).green()
    );
    if verbose {
        eprintln!("{}", format!("Total time: {:?}", start.elapsed()).dimmed());
    }
    restore_rebuild_pragmas(&conn, verbose)?;
    db::seal_staged_db(conn, staged.db_path())?;
    let publication = db::acquire_index_publication_guard(root)?;
    publication.install_staged(staged.db_path())?;
    Ok(())
}

/// Incrementally update the index
pub fn cmd_update(root: &Path, verbose: bool) -> Result<()> {
    let start = Instant::now();
    let _mutation_guard = db::acquire_rebuild_guard(root)?;

    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let _experimental_fast_rebuild_env = ScopedEnvVar::set_bool(
        "AST_INDEX_EXPERIMENTAL_FAST_REBUILD",
        crate::commands::try_is_experimental_fast_rebuild_enabled(root)?,
    );

    let mut conn = db::open_db_leased(root)?;

    // Load .ast-index.yaml so update honours the same include/exclude as rebuild.
    // Without this, update on a project with `include: [adfox, yabs/adfox]` would
    // walk the entire repo (e.g. all of a monorepo), hang indefinitely, and silently
    // pull in files outside the configured scope.
    let config = indexer::load_config(root).unwrap_or_default();
    let config_include = config.include.as_deref();
    let exclude_matcher = build_exclude_matcher(root, config.exclude.as_deref());

    if verbose {
        if let Some(inc) = config_include {
            eprintln!("[verbose] update include: {:?}", inc);
        }
        if let Some(ref exc) = config.exclude {
            eprintln!("[verbose] update exclude: {} patterns", exc.len());
        }
    }

    println!("{}", "Checking for changes...".cyan());
    let (updated, changed, deleted) = indexer::update_directory_incremental(
        &mut conn,
        root,
        true,
        config_include,
        exclude_matcher.as_ref(),
    )?;

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

    if verbose {
        eprintln!("\n{}", format!("Time: {:?}", start.elapsed()).dimmed());
    }
    Ok(())
}

/// Restore index from a .db file
pub fn cmd_restore(root: &Path, db_file: &str) -> Result<()> {
    let src = std::path::Path::new(db_file);

    // Serialize before allocating or populating a staging generation. This
    // also recovers validated staging directories abandoned by a dead writer.
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
    let dest = db::get_db_path(root)?;
    let dest_dir = dest
        .parent()
        .context("index database path has no parent directory")?;
    std::fs::create_dir_all(dest_dir)?;
    ensure_distinct_restore_source(src, &dest)?;

    let staged = IndexStaging::create(&dest, "restore")?;
    let normalized_root = db::normalize_root_for_storage(root);
    let stats = db::stage_restore_snapshot(src, staged.db_path(), &normalized_root)?;

    // Recheck under the exclusive mutation lock so a concurrently replaced
    // source can never become the live DB between validation and publication.
    ensure_distinct_restore_source(src, &dest)?;
    let publication = db::acquire_index_publication_guard(root)?;
    publication.install_staged(staged.db_path())?;

    println!("{}", format!("Restored index from: {}", db_file).green());
    println!("DB path: {}", dest.display());

    println!(
        "{}",
        format!(
            "Contains: {} files, {} symbols, {} refs",
            stats.file_count, stats.symbol_count, stats.refs_count
        )
        .dimmed()
    );

    Ok(())
}

struct IndexStaging {
    db_path: PathBuf,
    live_db: PathBuf,
}

impl IndexStaging {
    fn create(live_db: &Path, purpose: &str) -> Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let dest_dir = live_db
            .parent()
            .context("index database path has no parent directory")?;

        for _ in 0..128 {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let dir = dest_dir.join(format!(".{purpose}-{}-{id}", std::process::id()));
            match create_private_restore_dir(&dir) {
                Ok(()) => {
                    let db_path = dir.join("index.db");
                    if let Err(error) = db::register_index_staging(&db_path, live_db, purpose) {
                        let _ = std::fs::remove_dir(&dir);
                        return Err(error);
                    }
                    return Ok(Self {
                        db_path,
                        live_db: live_db.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "failed to create {purpose} staging directory {}",
                            dir.display()
                        )
                    })
                }
            }
        }

        anyhow::bail!(
            "failed to allocate a unique {purpose} staging directory in {}",
            dest_dir.display()
        )
    }

    fn db_path(&self) -> &Path {
        &self.db_path
    }
}

impl Drop for IndexStaging {
    fn drop(&mut self) {
        let _ = db::discard_index_staging(&self.db_path, &self.live_db);
    }
}

#[cfg(unix)]
fn create_private_restore_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_restore_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

fn ensure_distinct_restore_source(source: &Path, live_db: &Path) -> Result<()> {
    let source_metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect restore source {}", source.display()))?;
    anyhow::ensure!(
        source_metadata.file_type().is_file(),
        "restore source is not a regular file: {}",
        source.display()
    );

    let live_metadata = match std::fs::symlink_metadata(live_db) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect live index {}", live_db.display()))
        }
    };
    anyhow::ensure!(
        live_metadata.file_type().is_file(),
        "live index is not a regular file: {}",
        live_db.display()
    );
    anyhow::ensure!(
        !same_file_identity(&source_metadata, &live_metadata),
        "restore source is the live index database: {}",
        source.display()
    );
    Ok(())
}

#[cfg(unix)]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    left.file_attributes() == right.file_attributes()
        && left.creation_time() == right.creation_time()
        && left.last_write_time() == right.last_write_time()
        && left.file_size() == right.file_size()
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok().is_some()
        && left.modified().ok() == right.modified().ok()
}

/// Clear index database for current project
pub fn cmd_clear(root: &Path) -> Result<()> {
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
    db::clear_published_index(root)?;
    println!("Index cleared for {}", root.display());
    Ok(())
}

/// Show index statistics
pub fn cmd_stats(root: &Path, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;
    let stats = db::get_stats(&conn)?;
    let db_path = db::get_db_path(root)?;
    let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    if format == "json" {
        let result = serde_json::json!({
            "stats": stats,
            "db_size_bytes": db_size,
            "db_path": db_path.display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("{}", "Index Statistics:".bold());
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
pub fn cmd_add_root(root: &Path, path: &str, force: bool) -> Result<()> {
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
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

    // Overlap validation
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_new = std::path::Path::new(&abs_path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(&abs_path));

    if !force {
        if canonical_new.starts_with(&canonical_root) {
            println!(
                "{}",
                format!(
                    "Warning: '{}' is inside the project root '{}'. Files will be indexed twice.",
                    abs_path,
                    root.display()
                )
                .yellow()
            );
            println!("Use --force to add anyway, or use directory scoping instead.");
            return Ok(());
        }
        if canonical_root.starts_with(&canonical_new) {
            println!("{}", format!(
                "Warning: '{}' is a parent of the project root. This will cause massive duplication.",
                abs_path
            ).yellow());
            println!("Use --force to add anyway.");
            return Ok(());
        }
    }

    let conn = db::open_db_leased(root)?;
    db::add_extra_root(&conn, &abs_path)?;
    println!("{}", format!("Added source root: {}", abs_path).green());
    Ok(())
}

/// Remove an extra source root
pub fn cmd_remove_root(root: &Path, path: &str) -> Result<()> {
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
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

    let conn = db::open_db_leased(root)?;
    if db::remove_extra_root(&conn, &abs_path)? {
        println!("{}", format!("Removed source root: {}", abs_path).green());
    } else {
        println!("{}", format!("Root not found: {}", abs_path).yellow());
    }
    Ok(())
}

/// Legacy `list-roots` — lists subtrees by their canonical path only.
/// New code should prefer `subtree list`, which also shows names.
pub fn cmd_list_roots(root: &Path, format: &str) -> Result<()> {
    cmd_subtree_list(root, format)
}

fn ensure_index_exists(root: &Path) -> bool {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return false;
    }
    true
}

/// Resolve a user-supplied path into (canonical_path, original_path).
///
/// `canonical_path` is the absolute form used as `files.root_path` during
/// indexing. `original_path` preserves the user's input verbatim so a
/// project committed with relative paths stays portable across machines.
fn resolve_subtree_path(path: &str) -> (String, String) {
    let original = path.to_string();
    let candidate = if std::path::Path::new(path).is_absolute() {
        std::path::PathBuf::from(path)
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let canonical = db::safe_canonicalize(&candidate)
        .to_string_lossy()
        .into_owned();
    (canonical, original)
}

/// Reject obvious overlaps with the primary project root unless --force.
fn reject_overlap_with_root(root: &Path, canonical_new: &str, force: bool) -> Result<bool> {
    if force {
        return Ok(true);
    }
    let canonical_root = db::safe_canonicalize(root).to_string_lossy().into_owned();
    let canonical_new_pb = std::path::Path::new(canonical_new);
    let canonical_root_pb = std::path::Path::new(&canonical_root);

    if canonical_new_pb.starts_with(canonical_root_pb) {
        println!(
            "{}",
            format!(
                "Warning: '{}' is inside the project root '{}'. \
                 Files would be indexed twice.",
                canonical_new,
                root.display()
            )
            .yellow()
        );
        println!("Use --force to attach anyway, or scope via .ast-index.yaml `include`.");
        return Ok(false);
    }
    if canonical_root_pb.starts_with(canonical_new_pb) {
        println!(
            "{}",
            format!(
                "Warning: '{}' is a parent of the project root. \
                 This would cause massive duplication.",
                canonical_new
            )
            .yellow()
        );
        println!("Use --force to attach anyway.");
        return Ok(false);
    }
    Ok(true)
}

pub fn cmd_subtree_add(
    root: &Path,
    name: &str,
    path: &str,
    force: bool,
    format: &str,
) -> Result<()> {
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
    if !ensure_index_exists(root) {
        return Ok(());
    }
    let (canonical, original) = resolve_subtree_path(path);
    if !reject_overlap_with_root(root, &canonical, force)? {
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;
    if let Some(existing) = db::find_subtree_by_name(&conn, name)? {
        println!(
            "{}",
            format!(
                "Subtree name '{}' already attached to {}",
                name, existing.canonical_path
            )
            .yellow()
        );
        println!("Pick a different name, or remove the existing entry first.");
        return Ok(());
    }
    if let Some(existing) = db::find_subtree_by_root_path(&conn, &canonical)? {
        println!(
            "{}",
            format!(
                "This path is already attached as subtree '{}'",
                existing.name
            )
            .yellow()
        );
        return Ok(());
    }
    db::insert_subtree(&conn, name, &canonical, &original)?;

    if format == "json" {
        let payload = db::Subtree {
            name: name.to_string(),
            canonical_path: canonical.clone(),
            original_path: original.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "{}",
            format!("Attached subtree {} → {}", name, canonical).green()
        );
        if original != canonical {
            println!("  source: {}", original);
        }
    }
    Ok(())
}

pub fn cmd_subtree_remove(root: &Path, name: &str, format: &str) -> Result<()> {
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
    if !ensure_index_exists(root) {
        return Ok(());
    }
    let conn = db::open_db_leased(root)?;
    let existing = db::find_subtree_by_name(&conn, name)?;
    let removed = db::remove_subtree_by_name(&conn, name)?;

    if format == "json" {
        println!(
            "{}",
            serde_json::json!({ "removed": removed, "name": name }).to_string()
        );
        return Ok(());
    }

    if removed {
        if let Some(s) = existing {
            println!(
                "{}",
                format!("Detached subtree {} ({}).", name, s.canonical_path).green()
            );
        } else {
            println!("{}", format!("Detached subtree {}.", name).green());
        }
        println!("Run `ast-index rebuild` to drop its files from the index.");
    } else {
        println!("{}", format!("Subtree '{}' not found.", name).yellow());
    }
    Ok(())
}

pub fn cmd_subtree_list(root: &Path, format: &str) -> Result<()> {
    if !ensure_index_exists(root) {
        return Ok(());
    }
    let conn = db::open_db_leased(root)?;
    let subtrees = db::list_subtrees(&conn)?;

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&subtrees)?);
        return Ok(());
    }

    println!("{}", "Subtrees attached to this project:".bold());
    println!("  {} (primary)", root.display());
    if subtrees.is_empty() {
        println!(
            "  {}",
            "No extra subtrees attached. Use `ast-index subtree add <name> <path>` to add one."
                .dimmed()
        );
        return Ok(());
    }
    let name_w = subtrees.iter().map(|s| s.name.len()).max().unwrap_or(4);
    for s in &subtrees {
        let display = if s.original_path == s.canonical_path {
            s.canonical_path.clone()
        } else {
            format!("{} ({})", s.original_path, s.canonical_path)
        };
        println!("  {:<width$}  {}", s.name.cyan(), display, width = name_w);
    }
    Ok(())
}

/// Execute raw SQL query against the index database (SELECT only)
pub fn cmd_query(root: &Path, sql: &str, limit: usize) -> Result<()> {
    // Security: only allow SELECT statements
    let trimmed = sql.trim();
    let upper = trimmed.to_uppercase();
    if !upper.starts_with("SELECT") && !upper.starts_with("WITH") && !upper.starts_with("EXPLAIN") {
        anyhow::bail!("Only SELECT, WITH, and EXPLAIN queries are allowed");
    }
    // Block dangerous patterns
    for keyword in &[
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "ATTACH", "DETACH", "PRAGMA",
    ] {
        // Check that these keywords appear as statements, not inside strings
        if upper.contains(&format!(" {} ", keyword)) || upper.starts_with(&format!("{} ", keyword))
        {
            anyhow::bail!("Mutation queries are not allowed (found {})", keyword);
        }
    }

    let conn = db::open_db_leased(root)?;

    // Apply LIMIT if not already in query
    let query = if !upper.contains("LIMIT") {
        format!("{} LIMIT {}", trimmed.trim_end_matches(';'), limit)
    } else {
        trimmed.trim_end_matches(';').to_string()
    };

    let mut stmt = conn.prepare(&query)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let mut rows: Vec<serde_json::Value> = Vec::new();

    let mut result_rows = stmt.query([])?;
    while let Some(row) = result_rows.next()? {
        let mut obj = serde_json::Map::new();
        for (i, col_name) in column_names.iter().enumerate() {
            let val: serde_json::Value = match row.get_ref(i)? {
                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(n) => serde_json::json!(n),
                rusqlite::types::ValueRef::Real(f) => serde_json::json!(f),
                rusqlite::types::ValueRef::Text(s) => {
                    serde_json::Value::String(String::from_utf8_lossy(s).to_string())
                }
                rusqlite::types::ValueRef::Blob(b) => {
                    serde_json::Value::String(format!("<blob {} bytes>", b.len()))
                }
            };
            obj.insert(col_name.clone(), val);
        }
        rows.push(serde_json::Value::Object(obj));
    }

    let output = serde_json::json!({
        "columns": column_names,
        "rows": rows,
        "count": rows.len(),
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Print path to the SQLite index database
pub fn cmd_db_path(root: &Path) -> Result<()> {
    let db_path = db::get_db_path(root)?;
    println!("{}", db_path.display());
    Ok(())
}

/// Show database schema (tables and columns)
pub fn cmd_schema(root: &Path) -> Result<()> {
    let conn = db::open_db_leased(root)?;

    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%_fts%' ORDER BY name"
    )?;

    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut schema = serde_json::Map::new();

    for table in &tables {
        let mut cols_stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let columns: Vec<serde_json::Value> = cols_stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let col_type: String = row.get(2)?;
                let not_null: bool = row.get(3)?;
                let pk: bool = row.get(5)?;
                Ok(serde_json::json!({
                    "name": name,
                    "type": col_type,
                    "not_null": not_null,
                    "primary_key": pk,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Get row count
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {}", table), [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        schema.insert(
            table.clone(),
            serde_json::json!({
                "columns": columns,
                "row_count": count,
            }),
        );
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(schema))?
    );
    Ok(())
}
