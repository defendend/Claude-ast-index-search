//! Command implementations for kotlin-index CLI
//!
//! This module contains all command implementations:
//! - grep: Search commands (grep, find_class, find_file, etc.)
//! - management: Index management (rebuild, stats)
//! - index: File indexing operations
//! - modules: Module-related commands
//! - files: File operations (outline, stats)
//! - android: Android-specific (resources, strings)
//! - ios: iOS-specific commands
//! - perl: Perl-specific commands

pub mod grep;
pub mod management;
pub mod index;
pub mod modules;
pub mod files;
pub mod android;
pub mod ios;
pub mod perl;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use crossbeam_channel as channel;
use grep_regex::RegexMatcher;
use grep_searcher::{SearcherBuilder, sinks::UTF8};
use grep_searcher::MmapChoice;
use ignore::WalkBuilder;

use crate::db;

/// Check if no_ignore mode is enabled for this project
pub fn is_no_ignore_enabled(root: &Path) -> bool {
    if let Ok(conn) = db::open_db(root) {
        let result: Result<String, _> = conn.query_row(
            "SELECT value FROM metadata WHERE key = 'no_ignore'",
            [],
            |row| row.get(0),
        );
        return result.map(|v| v == "1").unwrap_or(false);
    }
    false
}

/// Get number of available CPU cores
pub fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Get relative path from root
pub fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// Fast parallel file search using grep-searcher and ignore crates
pub fn search_files<F>(root: &Path, pattern: &str, extensions: &[&str], mut handler: F) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    let matcher = RegexMatcher::new(pattern).context("Invalid regex pattern")?;
    let no_ignore = is_no_ignore_enabled(root);

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(!no_ignore)
        .git_exclude(!no_ignore)
        .threads(num_cpus())
        .build_parallel();

    // Use crossbeam for faster channel (bounded to prevent memory bloat)
    let (tx, rx) = channel::bounded(10000);

    // Use HashSet for O(1) extension lookup instead of O(n) linear search
    let extensions: Arc<HashSet<String>> = Arc::new(
        extensions.iter().map(|s| s.to_string()).collect()
    );

    walker.run(|| {
        let tx = tx.clone();
        let matcher = matcher.clone();
        let extensions = Arc::clone(&extensions);

        // Create optimized searcher ONCE per thread (not per file!)
        // SAFETY: memory-mapped files are safe when files aren't modified during search
        let mut searcher = SearcherBuilder::new()
            .memory_map(unsafe { MmapChoice::auto() })
            .line_number(true)
            .build();

        Box::new(move |entry| {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    // Fast O(1) HashSet lookup
                    if extensions.contains(ext.to_str().unwrap_or("")) {
                        let path_buf = path.to_path_buf();

                        let _ = searcher.search_path(
                            &matcher,
                            path,
                            UTF8(|line_num, line| {
                                let _ = tx.send((path_buf.clone(), line_num as usize, line.trim_end().to_string()));
                                Ok(true)
                            }),
                        );
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);

    for (path, line_num, line) in rx {
        handler(&path, line_num, &line);
    }

    Ok(())
}

/// Fast parallel file search with early termination support
#[allow(dead_code)]
pub fn search_files_limited<F>(
    root: &Path,
    pattern: &str,
    extensions: &[&str],
    limit: usize,
    mut handler: F,
) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    let matcher = RegexMatcher::new(pattern).context("Invalid regex pattern")?;
    let no_ignore = is_no_ignore_enabled(root);

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(!no_ignore)
        .git_exclude(!no_ignore)
        .threads(num_cpus())
        .build_parallel();

    let (tx, rx) = channel::bounded(limit.max(1000));

    let extensions: Arc<HashSet<String>> = Arc::new(
        extensions.iter().map(|s| s.to_string()).collect()
    );

    // Shared counter for early termination
    let found_count = Arc::new(AtomicUsize::new(0));
    let should_stop = Arc::new(AtomicBool::new(false));

    walker.run(|| {
        let tx = tx.clone();
        let matcher = matcher.clone();
        let extensions = Arc::clone(&extensions);
        let found_count = Arc::clone(&found_count);
        let should_stop = Arc::clone(&should_stop);

        // SAFETY: memory-mapped files are safe when files aren't modified during search
        let mut searcher = SearcherBuilder::new()
            .memory_map(unsafe { MmapChoice::auto() })
            .line_number(true)
            .build();

        Box::new(move |entry| {
            // Check early termination
            if should_stop.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }

            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if extensions.contains(ext.to_str().unwrap_or("")) {
                        let path_buf = path.to_path_buf();
                        let found_count = Arc::clone(&found_count);
                        let should_stop = Arc::clone(&should_stop);

                        let _ = searcher.search_path(
                            &matcher,
                            path,
                            UTF8(|line_num, line| {
                                // Check if we should stop
                                if should_stop.load(Ordering::Relaxed) {
                                    return Ok(false); // Stop searching this file
                                }

                                let count = found_count.fetch_add(1, Ordering::Relaxed);
                                if count >= limit {
                                    should_stop.store(true, Ordering::Relaxed);
                                    return Ok(false);
                                }

                                let _ = tx.send((path_buf.clone(), line_num as usize, line.trim_end().to_string()));
                                Ok(true)
                            }),
                        );
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);

    let mut count = 0;
    for (path, line_num, line) in rx {
        if count >= limit {
            break;
        }
        handler(&path, line_num, &line);
        count += 1;
    }

    Ok(())
}
