//! Watch mode — automatically update index on file changes

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use colored::Colorize;
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;

use crate::{db, indexer, parsers};

/// Watch for file changes and incrementally update the index
pub fn cmd_watch(root: &Path) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

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
                                "build" | "node_modules" | ".gradle" | ".git" | "target"
                                    | ".idea" | "__pycache__" | ".dart_tool"
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
    let mut conn = db::open_db(root)?;
    let (updated, changed, deleted) =
        indexer::update_directory_incremental(&mut conn, root, false)?;
    let _ = changed; // suppress unused
    Ok((updated, deleted))
}
