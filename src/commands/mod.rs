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

pub mod analysis;
pub mod android;
pub mod files;
pub mod grep;
pub mod index;
pub mod ios;
pub mod management;
pub mod modules;
pub mod perl;
pub mod project_info;
pub mod watch;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use crossbeam_channel as channel;
use grep_regex::RegexMatcher;
use grep_searcher::MmapChoice;
use grep_searcher::{sinks::UTF8, SearcherBuilder};
use ignore::WalkBuilder;
use rusqlite::Connection;

use crate::db;

/// Resolves stored relative paths to absolute paths when extra roots are configured.
///
/// The index stores paths relative to whichever root a file was discovered
/// under (primary or an extra root added via `add-root`). Without this, output
/// like `src/foo/Bar.java` is ambiguous — consumers can't tell whether to
/// look under the primary project or an extra root.
///
/// When no extra roots exist, [`resolve`] is a no-op so single-root output
/// stays byte-for-byte identical. When extras are configured, it probes each
/// root in order and returns the first absolute path that exists on disk.
pub struct PathResolver {
    primary: PathBuf,
    primary_key: String,
    extra: Vec<(String, PathBuf)>,
    /// Map from canonical_path → subtree name. Used by `decorate_for_display`
    /// to render `[name] /abs/path/file.rs` in text output. Empty when no
    /// named subtrees are attached, in which case `decorate_for_display`
    /// falls back to the bare resolved path.
    subtree_names: Vec<(String, String)>,
    /// When `true`, `resolve` and `resolve_with_root` prefix subtree-owned
    /// paths with `[name] `. Used for text output; disabled in JSON mode
    /// so downstream tooling sees raw absolute paths.
    decorate_subtrees: bool,
}

impl PathResolver {
    pub fn from_conn(primary: &Path, conn: &Connection) -> Self {
        let primary_key = db::normalize_root_for_storage(primary);
        let extra = db::get_extra_roots(conn)
            .unwrap_or_default()
            .into_iter()
            .map(|root| {
                let path = PathBuf::from(&root);
                (db::normalize_root_for_storage(&path), path)
            })
            .collect();
        let subtree_names = db::list_subtrees(conn)
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.canonical_path, s.name))
            .collect();
        // Default to text-mode decoration when the env hint says so. main.rs
        // exports AST_INDEX_FORMAT right after clap parse so we don't have
        // to thread `format` through every command signature.
        let decorate_subtrees = std::env::var("AST_INDEX_FORMAT")
            .map(|v| v != "json")
            .unwrap_or(true);
        Self {
            primary: primary.to_path_buf(),
            primary_key,
            extra,
            subtree_names,
            decorate_subtrees,
        }
    }

    /// Toggle subtree-name decoration on resolved paths. Call sites pass
    /// `format != "json"` so that text output gets `[name] /abs/path` while
    /// structured JSON output stays raw.
    pub fn with_decoration(mut self, decorate: bool) -> Self {
        self.decorate_subtrees = decorate;
        self
    }

    /// Return the subtree name owning the given `root_path`, if any.
    /// `None` when the file belongs to the primary project or when no named
    /// subtrees are attached.
    pub fn subtree_name(&self, root_path: Option<&str>) -> Option<&str> {
        let root = root_path?;
        if root == self.primary_key {
            return None;
        }
        self.subtree_names
            .iter()
            .find(|(canon, _)| canon == root)
            .map(|(_, name)| name.as_str())
    }

    /// Format a path for human-readable output: prefixes `[name] ` when the
    /// file belongs to a named subtree, otherwise returns the resolved
    /// absolute path unchanged. Use for text output; structured JSON output
    /// keeps the raw absolute path so downstream tooling doesn't have to
    /// parse the prefix.
    pub fn decorate_for_display(&self, rel: &str, root_path: Option<&str>) -> String {
        let resolved = self.resolve_with_root(rel, root_path);
        match self.subtree_name(root_path) {
            Some(name) => format!("[{}] {}", name, resolved),
            None => resolved,
        }
    }

    /// Wrap a resolved path with `[name] ` when decoration is on and the
    /// file's owning root maps to a named subtree.
    fn maybe_decorate(&self, resolved: String, root_path: Option<&str>) -> String {
        if !self.decorate_subtrees {
            return resolved;
        }
        match self.subtree_name(root_path) {
            Some(name) => format!("[{}] {}", name, resolved),
            None => resolved,
        }
    }

    /// Absolute path of a stored relative path. Returns `rel` unchanged when
    /// no extra roots are configured; otherwise probes primary then each
    /// extra root and returns the first match on disk. Falls back to `rel`
    /// as-is if no root contains the file (stale index), so output never
    /// lies about a file's location.
    pub fn resolve(&self, rel: &str) -> String {
        if self.extra.is_empty() {
            return rel.to_string();
        }
        for root in std::iter::once(&self.primary).chain(self.extra.iter().map(|(_, path)| path)) {
            let abs = root.join(rel);
            if abs.exists() {
                return abs.to_string_lossy().into_owned();
            }
        }
        rel.to_string()
    }

    /// Absolute path of a stored relative path when the owning root is known.
    /// Falls back to generic probing when the hint is absent or stale.
    /// Applies subtree decoration when enabled via `with_decoration(true)`.
    pub fn resolve_with_root(&self, rel: &str, root_path: Option<&str>) -> String {
        let raw = self.resolve_with_root_raw(rel, root_path);
        self.maybe_decorate(raw, root_path)
    }

    /// Raw version of `resolve_with_root` — never decorates, always returns
    /// just the absolute path. Useful when callers want the path for both
    /// text and JSON output and apply decoration themselves.
    pub fn resolve_with_root_raw(&self, rel: &str, root_path: Option<&str>) -> String {
        if self.extra.is_empty() {
            return rel.to_string();
        }

        if let Some(root_path) = root_path {
            if root_path == self.primary_key {
                let abs = self.primary.join(rel);
                if abs.exists() {
                    return abs.to_string_lossy().into_owned();
                }
                return self.resolve(rel);
            }
            if let Some((_, root)) = self.extra.iter().find(|(key, _)| key == root_path) {
                let abs = root.join(rel);
                if abs.exists() {
                    return abs.to_string_lossy().into_owned();
                }
                return self.resolve(rel);
            }
            let abs = PathBuf::from(root_path).join(rel);
            if abs.exists() {
                return abs.to_string_lossy().into_owned();
            }
        }

        self.resolve(rel)
    }
}

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

/// Check if the last rebuild for this project used experimental fast rebuild mode.
pub fn is_experimental_fast_rebuild_enabled(root: &Path) -> bool {
    if let Ok(conn) = db::open_db(root) {
        return db::is_experimental_fast_rebuild_enabled_in_db(&conn);
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
pub fn search_files<F>(
    root: &Path,
    pattern: &str,
    extensions: &[&str],
    mut handler: F,
) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    let matcher = RegexMatcher::new(pattern).context("Invalid regex pattern")?;
    let no_ignore = is_no_ignore_enabled(root);
    let use_git = crate::indexer::has_git_repo(root) && !no_ignore;
    let arc_root = if no_ignore {
        None
    } else {
        crate::indexer::find_arc_root(root)
    };

    let mut wb = WalkBuilder::new(root);
    wb.hidden(true)
        .git_ignore(use_git)
        .git_exclude(use_git)
        .filter_entry(|entry| !crate::indexer::is_excluded_dir(entry))
        .threads(num_cpus());
    if let Some(ref arc) = arc_root {
        wb.add_custom_ignore_filename(".gitignore");
        wb.add_custom_ignore_filename(".arcignore");
        let root_gitignore = arc.join(".gitignore");
        if root_gitignore.exists() {
            wb.add_ignore(root_gitignore);
        }
    }
    let walker = wb.build_parallel();

    // Use crossbeam for faster channel (bounded to prevent memory bloat)
    let (tx, rx) = channel::bounded::<(Arc<Path>, usize, String)>(10000);

    // Use HashSet for O(1) extension lookup instead of O(n) linear search
    let extensions: Arc<HashSet<String>> =
        Arc::new(extensions.iter().map(|s| s.to_string()).collect());

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
                        let path_arc: Arc<Path> = Arc::from(path);

                        let _ = searcher.search_path(
                            &matcher,
                            path,
                            UTF8(|line_num, line| {
                                let _ = tx.send((
                                    Arc::clone(&path_arc),
                                    line_num as usize,
                                    line.trim_end().to_string(),
                                ));
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
    let use_git = crate::indexer::has_git_repo(root) && !no_ignore;
    let arc_root = if no_ignore {
        None
    } else {
        crate::indexer::find_arc_root(root)
    };

    let mut wb = WalkBuilder::new(root);
    wb.hidden(true)
        .git_ignore(use_git)
        .git_exclude(use_git)
        .filter_entry(|entry| !crate::indexer::is_excluded_dir(entry))
        .threads(num_cpus());
    if let Some(ref arc) = arc_root {
        wb.add_custom_ignore_filename(".gitignore");
        wb.add_custom_ignore_filename(".arcignore");
        let root_gitignore = arc.join(".gitignore");
        if root_gitignore.exists() {
            wb.add_ignore(root_gitignore);
        }
    }
    let walker = wb.build_parallel();

    let (tx, rx) = channel::bounded::<(Arc<Path>, usize, String)>(limit.max(1000));

    let extensions: Arc<HashSet<String>> =
        Arc::new(extensions.iter().map(|s| s.to_string()).collect());

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
                        let path_arc: Arc<Path> = Arc::from(path);
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

                                let _ = tx.send((
                                    Arc::clone(&path_arc),
                                    line_num as usize,
                                    line.trim_end().to_string(),
                                ));
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
