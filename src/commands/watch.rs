//! Watch mode — automatically update index on file changes

use std::io::Write;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use colored::Colorize;
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;

use crate::commands::{self, management::ScopedEnvVar};
use crate::{db, indexer, parsers};

fn open_watch_lock(root: &Path) -> Result<std::fs::File> {
    let lock_path = db::get_db_path(root)?.with_extension("watch.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(Into::into)
}

/// Acquire an exclusive lock for watch mode, scoped to the resolved project
/// database. The lock remains held until the returned file is dropped.
fn try_acquire_watch_lock(root: &Path) -> Result<Option<std::fs::File>> {
    use fs2::FileExt;
    let file = open_watch_lock(root)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            file.set_len(0)?;
            let mut f = &file;
            write!(f, "{}", std::process::id())?;
            Ok(Some(file))
        }
        Err(error) if db::lock_is_contended(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Report whether this project's watch lock is currently held. This probes
/// the same lock as [`cmd_watch`], so another project's watcher cannot affect
/// the result.
fn is_watch_running(root: &Path) -> Result<bool> {
    use fs2::FileExt;
    let file = open_watch_lock(root)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(false),
        Err(error) if db::lock_is_contended(&error) => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// Print a stable watcher status. Callers that only need the exit status use
/// `--quiet`; the CLI exits successfully only while this project is watched.
pub fn cmd_watch_status(root: &Path, quiet: bool, format: &str) -> Result<bool> {
    let watching = is_watch_running(root)?;
    if !quiet {
        if format == "json" {
            println!(r#"{{"watching":{watching}}}"#);
        } else if watching {
            println!("watching");
        } else {
            println!("not-watching");
        }
        std::io::stdout().flush()?;
    }
    Ok(watching)
}

/// Watch for file changes and incrementally update the index
pub fn cmd_watch(root: &Path) -> Result<()> {
    // Held for the complete watch loop. The lease file lives outside the
    // project cache directory, so stale-cache GC cannot unlink this index
    // while the watcher is idle between SQLite connections.
    let _cache_lease = db::acquire_project_lease(root)?;
    let Some(initial) = db::open_existing_db_leased(root)? else {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    };
    drop(initial);

    // Ensure only one watch process runs at a time
    let _lock = match try_acquire_watch_lock(root)? {
        Some(lock) => lock,
        None => {
            eprintln!("{}", "Another ast-index watch is already running.".yellow());
            return Ok(());
        }
    };

    println!(
        "{}",
        format!("Watching for changes in {}...", root.display()).cyan()
    );
    println!("{}", "Press Ctrl+C to stop.".dimmed());

    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
    debouncer.watcher().watch(root, RecursiveMode::Recursive)?;

    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                let changed: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        let path = &e.path;
                        // Only process supported source files
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if !parsers::is_supported_extension(ext) {
                                return false;
                            }
                        } else {
                            return false;
                        }
                        // Skip excluded directories
                        !path.components().any(|c| {
                            let s = c.as_os_str().to_str().unwrap_or("");
                            matches!(
                                s,
                                "build"
                                    | "node_modules"
                                    | ".gradle"
                                    | ".git"
                                    | "target"
                                    | ".idea"
                                    | "__pycache__"
                                    | ".dart_tool"
                            )
                        })
                    })
                    .collect();

                if changed.is_empty() {
                    continue;
                }

                let start = Instant::now();
                let file_count = changed.len();
                eprintln!(
                    "{}",
                    format!("Detected {} changed file(s), updating...", file_count).yellow()
                );

                match update_index(root) {
                    Ok((updated, deleted)) => {
                        if updated > 0 || deleted > 0 {
                            eprintln!(
                                "{}",
                                format!(
                                    "Updated {} files, deleted {} ({:?})",
                                    updated,
                                    deleted,
                                    start.elapsed()
                                )
                                .green()
                            );
                        } else {
                            eprintln!(
                                "{}",
                                format!("No index changes ({:?})", start.elapsed()).dimmed()
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("{}", format!("Update error: {}", e).red());
                    }
                }
            }
            Ok(Err(err)) => {
                eprintln!("{}", format!("Watch error: {}", err).red());
            }
            Err(e) => {
                eprintln!("{}", format!("Channel error: {}", e).red());
                break;
            }
        }
    }

    Ok(())
}

fn update_index(root: &Path) -> Result<(usize, usize)> {
    // Watch is long-lived, so take the common mutation lock only for one
    // coalesced update batch. Readers remain concurrent through SQLite WAL.
    let _mutation_guard = db::acquire_rebuild_guard(root)?;
    let _experimental_fast_rebuild_env = ScopedEnvVar::set_bool(
        "AST_INDEX_EXPERIMENTAL_FAST_REBUILD",
        commands::try_is_experimental_fast_rebuild_enabled(root)?,
    );

    let mut conn = db::open_existing_db_leased(root)?
        .ok_or_else(|| anyhow::anyhow!("Index was cleared; run 'ast-index rebuild' first."))?;

    // Honour .ast-index.yaml so watch stays scoped to the same paths as rebuild/update.
    let config = indexer::load_config(root).unwrap_or_default();
    let config_include = config.include.as_deref();
    let exclude_matcher: Option<ignore::gitignore::Gitignore> = config
        .exclude
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(|patterns| {
            let mut gb = ignore::gitignore::GitignoreBuilder::new(root);
            for p in patterns {
                gb.add_line(None, p).ok();
            }
            gb.build().ok()
        })
        .flatten();

    let (updated, changed, deleted) = indexer::update_directory_incremental(
        &mut conn,
        root,
        false,
        config_include,
        exclude_matcher.as_ref(),
    )?;
    let _ = changed; // suppress unused
    Ok((updated, deleted))
}
