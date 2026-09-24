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
pub mod changed;
pub mod explore;
pub mod files;
pub mod git_signals;
pub mod graph;
pub mod grep;
pub mod index;
pub mod ios;
pub mod management;
pub mod modules;
pub mod perl;
pub mod project_info;
pub mod rank;
pub mod test_paths;
pub mod watch;

pub use test_paths::is_test_path;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use colored::Colorize;
use crossbeam_channel as channel;
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use grep_searcher::MmapChoice;
use grep_searcher::{
    sinks::{Bytes, UTF8},
    Searcher, SearcherBuilder, Sink,
};
use ignore::WalkBuilder;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::db;

pub const PAGINATED_JSON_SCHEMA_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Pagination {
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub limit: usize,
}

impl Pagination {
    pub fn new(total: usize, returned: usize, limit: usize) -> Self {
        Self {
            total,
            returned,
            truncated: total > returned,
            limit,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub schema_version: u8,
    pub items: Vec<T>,
    pub pagination: Pagination,
}

impl<T> Page<T> {
    pub fn new(mut items: Vec<T>, total: usize, limit: usize) -> Self {
        items.truncate(limit);
        let pagination = Pagination::new(total, items.len(), limit);
        Self {
            schema_version: PAGINATED_JSON_SCHEMA_VERSION,
            items,
            pagination,
        }
    }
}

pub fn print_truncation_notice(pagination: Pagination) {
    if pagination.truncated {
        println!(
            "  {}",
            format!(
                "Truncated: showing {} of {} results; use --limit {} to see all.",
                pagination.returned, pagination.total, pagination.total
            )
            .yellow()
        );
    }
}

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
    /// Build a resolver with the historical best-effort behaviour. Database
    /// errors fall back to a primary-only resolver.
    pub fn from_conn(primary: &Path, conn: &Connection) -> Self {
        Self::try_from_conn(primary, conn)
            .unwrap_or_else(|_| Self::from_subtrees(primary, Vec::new()))
    }

    /// Build a resolver while propagating subtree metadata errors.
    pub fn try_from_conn(primary: &Path, conn: &Connection) -> Result<Self> {
        // Direct pre-3.47 connections have not gone through open_db's eager
        // migration. The compatibility shim upgrades metadata.extra_roots
        // transactionally before we read the named subtree rows.
        db::get_extra_roots(conn)?;
        Ok(Self::from_subtrees(primary, db::list_subtrees(conn)?))
    }

    fn from_subtrees(primary: &Path, subtrees: Vec<db::Subtree>) -> Self {
        let primary_key = db::normalize_root_for_storage(primary);
        let extra = subtrees
            .iter()
            .map(|subtree| {
                let path = PathBuf::from(&subtree.canonical_path);
                (db::normalize_root_for_storage(&path), path)
            })
            .collect();
        let subtree_names = subtrees
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
    /// Directories a grep-based command must walk to see the same files the
    /// index does: the primary root plus attached subtrees, narrowed by
    /// `--subtree NAME` / `--local` exactly like the SQL-backed commands.
    /// Pure: reads only resolver state, never the database, so it is safe to
    /// call while the caller already holds a connection.
    pub fn grep_roots(&self) -> Vec<PathBuf> {
        if std::env::var_os("AST_INDEX_LOCAL_SCOPE").is_some() {
            return vec![self.primary.clone()];
        }
        if let Ok(name) = std::env::var("AST_INDEX_SUBTREE") {
            return self
                .subtree_names
                .iter()
                .filter(|(_, n)| *n == name)
                .map(|(canon, _)| PathBuf::from(canon))
                .filter(|p| p.is_dir())
                .collect();
        }
        let mut roots = vec![self.primary.clone()];
        for (_, path) in &self.extra {
            if path.is_dir() && !roots.contains(path) {
                roots.push(path.clone());
            }
        }
        roots
    }

    /// Whether a stored `root_path` belongs to the primary project root
    /// rather than to an attached subtree.
    pub fn is_primary_root(&self, root_path: Option<&str>) -> bool {
        root_path.map_or(true, |root| root == self.primary_key)
    }

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

    /// Does the file at this `root_path` satisfy the user's `--subtree NAME`
    /// or `--local` filter for the current run?
    ///
    /// `--subtree NAME` keeps only rows whose subtree name matches (case
    /// sensitive). `--local` drops every named-subtree row, keeping only
    /// primary-project files. When neither env hint is set, everything
    /// passes.
    pub fn matches_filter(&self, root_path: Option<&str>) -> bool {
        if std::env::var("AST_INDEX_LOCAL_SCOPE").is_ok() {
            return self.subtree_name(root_path).is_none();
        }
        if let Ok(name) = std::env::var("AST_INDEX_SUBTREE") {
            return self.subtree_name(root_path) == Some(name.as_str());
        }
        true
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
    try_is_no_ignore_enabled(root).unwrap_or(false)
}

/// Strict variant of [`is_no_ignore_enabled`] for production command paths.
pub fn try_is_no_ignore_enabled(root: &Path) -> Result<bool> {
    let Some(_cache_lease) = db::acquire_project_lease_if_initialized(root)? else {
        return Ok(false);
    };
    let conn = db::open_db_leased(root)?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'no_ignore'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read metadata.no_ignore")?;
    Ok(value.as_deref() == Some("1"))
}

/// Check if the last rebuild for this project used experimental fast rebuild mode.
pub fn is_experimental_fast_rebuild_enabled(root: &Path) -> bool {
    try_is_experimental_fast_rebuild_enabled(root).unwrap_or(false)
}

/// Strict variant of [`is_experimental_fast_rebuild_enabled`] for production commands.
pub fn try_is_experimental_fast_rebuild_enabled(root: &Path) -> Result<bool> {
    let Some(_cache_lease) = db::acquire_project_lease_if_initialized(root)? else {
        return Ok(false);
    };
    let conn = db::open_db_leased(root)?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'experimental_fast_rebuild'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read metadata.experimental_fast_rebuild")?;
    Ok(value.as_deref() == Some("1"))
}

/// Get number of available CPU cores
pub fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Get relative path from root
/// Path for grep-based output: relative to the primary root when the file
/// lives there, otherwise the subtree-decorated absolute path (`[name] /abs`)
/// that the SQL-backed commands print. `relative_path` alone would yield
/// `../../other/file.rs` for a subtree file, which is useless to a reader.
pub fn display_path(resolver: &PathResolver, root: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(root) {
        return rel.to_string_lossy().into_owned();
    }
    let abs = path.to_string_lossy();
    for (key, subtree_root) in &resolver.extra {
        if path.starts_with(subtree_root) {
            return resolver.maybe_decorate(abs.into_owned(), Some(key));
        }
    }
    abs.into_owned()
}

pub fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// Fast parallel file search using grep-searcher and ignore crates. Like
/// every grep-based walk here, it never reads minified files.
pub fn search_files<F>(root: &Path, pattern: &str, extensions: &[&str], handler: F) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    search_files_in(
        root,
        std::slice::from_ref(&root.to_path_buf()),
        pattern,
        extensions,
        handler,
    )
}

/// `search_files` over several roots in one parallel walk. `root` is still
/// the primary project (ignore rules and VCS detection come from it); `roots`
/// lists every directory to scan, typically the primary plus attached
/// subtrees from [`PathResolver::grep_roots`].
pub fn search_files_in<F>(
    root: &Path,
    roots: &[PathBuf],
    pattern: &str,
    extensions: &[&str],
    handler: F,
) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    search_files_in_kept(
        root,
        roots,
        pattern,
        extensions,
        None,
        &|_, _| true,
        handler,
    )
}

/// The words of every indexed file of the primary root, for telling files
/// that cannot contain a literal apart without opening them: see
/// [`crate::indexer::content_words`].
pub struct WordIndex {
    root: PathBuf,
    /// Primary-root relative path -> (mtime, size, words) as indexed.
    files: HashMap<String, (i64, i64, String)>,
}

impl WordIndex {
    /// `None` when the index keeps no words.
    pub fn load(root: &Path, conn: &Connection) -> Result<Option<Self>> {
        let root_key = db::normalize_root_for_storage(root);
        let Some(rows) = db::load_file_words(conn, &root_key)? else {
            return Ok(None);
        };
        let files = rows
            .into_iter()
            .map(|(path, mtime, size, words)| (path, (mtime, size, words)))
            .collect();
        Ok(Some(Self {
            root: root.to_path_buf(),
            files,
        }))
    }

    /// A filter for files that may contain one of `literals`. `None` when a
    /// literal has no word runs (`->`), since nothing can be skipped then.
    pub fn prefilter(&self, literals: &[&str]) -> Option<WordPrefilter<'_>> {
        let mut runs: Vec<String> = Vec::new();
        let mut literal_runs = Vec::new();
        for literal in literals {
            let own = crate::indexer::literal_word_runs(literal);
            if own.is_empty() {
                return None;
            }
            literal_runs.push(
                own.into_iter()
                    .map(|run| match runs.iter().position(|known| *known == run) {
                        Some(at) => at,
                        None => {
                            runs.push(run);
                            runs.len() - 1
                        }
                    })
                    .collect::<Vec<_>>(),
            );
        }
        if literal_runs.is_empty() {
            return None;
        }
        let runs = regex::RegexSet::new(runs.iter().map(|run| regex::escape(run))).ok()?;
        Some(WordPrefilter {
            index: self,
            runs,
            literal_runs,
        })
    }
}

/// Files of a [`WordIndex`] that cannot contain any of some literals.
///
/// A file is skipped only while it is exactly the version the index read
/// (same mtime and size) and, for every literal, one of the literal's word
/// runs occurs in none of its words. Files the index does not hold, changed
/// files and files under attached subtrees are searched as before, so a
/// search over the tree finds what it found without the filter.
pub struct WordPrefilter<'a> {
    index: &'a WordIndex,
    runs: regex::RegexSet,
    /// For each literal, the indices of its runs in `runs`.
    literal_runs: Vec<Vec<usize>>,
}

impl WordPrefilter<'_> {
    /// Whether `path` has to be searched.
    pub fn may_contain(&self, path: &Path) -> bool {
        let Some(rel) = path
            .strip_prefix(&self.index.root)
            .ok()
            .and_then(Path::to_str)
        else {
            return true;
        };
        let Some((mtime, size, words)) = self.index.files.get(rel) else {
            return true;
        };
        let found = self.runs.matches(words);
        if self
            .literal_runs
            .iter()
            .any(|runs| runs.iter().all(|&run| found.matched(run)))
        {
            return true;
        }
        let Ok(metadata) = std::fs::metadata(path) else {
            return true;
        };
        let current_mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        current_mtime != Some(*mtime) || metadata.len() as i64 != *size
    }
}

/// [`search_files_in`] with a line filter that runs on the search threads.
///
/// `keep` sees the same trimmed line `handler` would, and a line it rejects
/// never reaches `handler`. Filtering there instead of in `handler` spreads a
/// costly per-line check (a capturing regex) over every search thread rather
/// than serialising it on the one thread that drains the results.
pub fn search_files_in_kept<F>(
    root: &Path,
    roots: &[PathBuf],
    pattern: &str,
    extensions: &[&str],
    prefilter: Option<&WordPrefilter<'_>>,
    keep: &(dyn Fn(&Path, &str) -> bool + Sync),
    mut handler: F,
) -> Result<()>
where
    F: FnMut(&Path, usize, &str),
{
    let matcher = RegexMatcher::new(pattern).context("Invalid regex pattern")?;
    let no_ignore = try_is_no_ignore_enabled(root)?;
    let use_git = crate::indexer::has_git_repo(root) && !no_ignore;
    let arc_root = if no_ignore {
        None
    } else {
        crate::indexer::find_arc_root(root)
    };

    let first = roots.first().map(PathBuf::as_path).unwrap_or(root);
    let mut wb = WalkBuilder::new(first);
    for extra in roots.iter().skip(1) {
        wb.add(extra);
    }
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

    std::thread::scope(|scope| -> Result<()> {
        let worker = scope.spawn(move || {
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
                            if extensions.contains(ext.to_str().unwrap_or(""))
                                && prefilter.map_or(true, |filter| filter.may_contain(path))
                            {
                                let path_arc: Arc<Path> = Arc::from(path);

                                search_source_file(
                                    &mut searcher,
                                    &matcher,
                                    path,
                                    UTF8(|line_num, line| {
                                        let line = line.trim_end();
                                        if !keep(path, line) {
                                            return Ok(true);
                                        }
                                        if tx
                                            .send((
                                                Arc::clone(&path_arc),
                                                line_num as usize,
                                                line.to_string(),
                                            ))
                                            .is_err()
                                        {
                                            return Ok(false);
                                        }
                                        Ok(true)
                                    }),
                                );
                            }
                        }
                    }
                    ignore::WalkState::Continue
                })
            });
        });

        for (path, line_num, line) in rx {
            handler(&path, line_num, &line);
        }
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("parallel file search worker panicked"))?;
        Ok(())
    })?;

    Ok(())
}

/// Scan file matches, apply caller-side filtering before pagination, and
/// retain an exact total without storing every accepted result. This is used
/// by commands whose validity checks (for example, excluding definitions)
/// cannot safely be applied by the regex scanner itself.
pub fn search_files_page<T, F>(
    root: &Path,
    pattern: &str,
    extensions: &[&str],
    limit: usize,
    filter_map: F,
) -> Result<Page<T>>
where
    F: FnMut(&Path, usize, &str) -> Option<T>,
{
    search_files_page_in(
        root,
        std::slice::from_ref(&root.to_path_buf()),
        pattern,
        extensions,
        limit,
        filter_map,
    )
}

/// [`search_files_page`] skipping files `prefilter` rules out.
pub fn search_files_page_prefiltered<T, F>(
    root: &Path,
    pattern: &str,
    extensions: &[&str],
    limit: usize,
    prefilter: Option<&WordPrefilter<'_>>,
    filter_map: F,
) -> Result<Page<T>>
where
    F: FnMut(&Path, usize, &str) -> Option<T>,
{
    search_files_page_in_kept(
        root,
        std::slice::from_ref(&root.to_path_buf()),
        pattern,
        extensions,
        limit,
        prefilter,
        &|_, _| true,
        filter_map,
    )
}

/// `search_files_page` over several roots; see [`search_files_in`].
pub fn search_files_page_in<T, F>(
    root: &Path,
    roots: &[PathBuf],
    pattern: &str,
    extensions: &[&str],
    limit: usize,
    filter_map: F,
) -> Result<Page<T>>
where
    F: FnMut(&Path, usize, &str) -> Option<T>,
{
    search_files_page_in_kept(
        root,
        roots,
        pattern,
        extensions,
        limit,
        None,
        &|_, _| true,
        filter_map,
    )
}

/// [`search_files_page_in`] with a `keep` filter run on the search threads;
/// see [`search_files_in_kept`].
#[allow(clippy::too_many_arguments)]
pub fn search_files_page_in_kept<T, F>(
    root: &Path,
    roots: &[PathBuf],
    pattern: &str,
    extensions: &[&str],
    limit: usize,
    prefilter: Option<&WordPrefilter<'_>>,
    keep: &(dyn Fn(&Path, &str) -> bool + Sync),
    mut filter_map: F,
) -> Result<Page<T>>
where
    F: FnMut(&Path, usize, &str) -> Option<T>,
{
    let mut items = Vec::with_capacity(limit.min(1024));
    let mut total = 0usize;
    search_files_in_kept(
        root,
        roots,
        pattern,
        extensions,
        prefilter,
        keep,
        |path, line_num, line| {
            if let Some(item) = filter_map(path, line_num, line) {
                total = total.saturating_add(1);
                if items.len() < limit {
                    items.push(item);
                }
            }
        },
    )?;
    Ok(Page::new(items, total, limit))
}

/// Runs `searcher` over `path` unless it is minified. A file type minifiers
/// emit is read once, and the same bytes are both judged and searched.
fn search_source_file<S: Sink>(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: &Path,
    sink: S,
) {
    if crate::minified::judged_by_content(path) {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        if !crate::minified::skip(path, Some(&bytes)) {
            let _ = searcher.search_slice(matcher, &bytes, sink);
        }
    } else if !crate::minified::skip_by_name(path) {
        let _ = searcher.search_path(matcher, path, sink);
    }
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
    let walker = project_walker(root)?;

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

                        search_source_file(
                            &mut searcher,
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

/// Every file under `root` with one of `extensions`, in path order, under the
/// ignore rules the indexer applies.
///
/// The tree is walked in parallel and sorted afterwards, so the order does not
/// depend on which thread reached a file first.
pub fn project_source_files(root: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>> {
    let walker = project_walker(root)?;
    let extensions: HashSet<&str> = extensions.iter().copied().collect();
    let (tx, rx) = channel::unbounded::<PathBuf>();
    walker.run(|| {
        let tx = tx.clone();
        let extensions = &extensions;
        Box::new(move |entry| {
            if let Ok(entry) = entry {
                let wanted = entry
                    .path()
                    .extension()
                    .is_some_and(|ext| extensions.contains(ext.to_str().unwrap_or("")));
                if wanted {
                    let _ = tx.send(entry.into_path());
                }
            }
            ignore::WalkState::Continue
        })
    });
    drop(tx);
    let mut files: Vec<PathBuf> = rx.into_iter().collect();
    files.sort_unstable();
    Ok(files)
}

/// A line one of the [`search_files_limited_each`] patterns matched.
struct PatternHit {
    pattern: usize,
    line_num: usize,
    text: String,
}

/// Several searches over `files` answered by one pass, with a fixed outcome.
///
/// Each of `patterns` gets the first `limit` lines that it matches and `keep`
/// accepts, taking `files` in the given order and lines in file order, and
/// `handler` receives them in that order with the pattern's index. Filtering
/// through `keep` rather than in `handler` makes `limit` count only the lines
/// the caller wants.
///
/// Files are searched in parallel, but a file's lines are handed over only
/// once every file before it has been searched, so the outcome does not depend
/// on which thread finished first. Files stop being searched once every
/// pattern has its lines.
///
/// The search is for `candidates`, which must match every line any of the
/// patterns matches; each candidate line is then tested against the patterns
/// one by one. A pattern comes with a literal that all of its matches contain,
/// checked first because a substring test is far cheaper than the pattern.
///
/// Minified files among `files` are passed over without a hit.
pub fn search_files_limited_each<K, F>(
    files: &[PathBuf],
    candidates: &str,
    patterns: &[(String, String)],
    limit: usize,
    keep: K,
    handler: F,
) -> Result<()>
where
    K: Fn(usize, &Path, &str) -> bool + Sync,
    F: FnMut(usize, &Path, usize, &str),
{
    search_files_limited_each_prefiltered(files, candidates, patterns, limit, None, keep, handler)
}

/// [`search_files_limited_each`] skipping, without opening them, the files
/// `prefilter` rules out. A skipped file counts as searched with no lines,
/// so the order in which lines are taken stays the same.
pub fn search_files_limited_each_prefiltered<K, F>(
    files: &[PathBuf],
    candidates: &str,
    patterns: &[(String, String)],
    limit: usize,
    prefilter: Option<&WordPrefilter<'_>>,
    keep: K,
    mut handler: F,
) -> Result<()>
where
    K: Fn(usize, &Path, &str) -> bool + Sync,
    F: FnMut(usize, &Path, usize, &str),
{
    let matcher = RegexMatcher::new(candidates).context("Invalid regex pattern")?;
    let exact = patterns
        .iter()
        .map(|(pattern, _)| RegexMatcher::new(pattern).context("Invalid regex pattern"))
        .collect::<Result<Vec<_>>>()?;
    let required = patterns
        .iter()
        .map(|(_, literal)| regex::bytes::Regex::new(&regex::escape(literal)))
        .collect::<Result<Vec<_>, _>>()?;
    if patterns.is_empty() || limit == 0 || files.is_empty() {
        return Ok(());
    }

    let satisfied: Vec<AtomicBool> = patterns.iter().map(|_| AtomicBool::new(false)).collect();
    let stop = AtomicBool::new(false);
    let next = AtomicUsize::new(0);
    let (tx, rx) = channel::bounded::<(usize, Vec<PatternHit>)>(1024);

    std::thread::scope(|scope| -> Result<()> {
        let mut workers = Vec::new();
        for _ in 0..num_cpus().min(files.len()) {
            let tx = tx.clone();
            let (matcher, exact, required) = (&matcher, &exact, &required);
            let (satisfied, stop, next, keep) = (&satisfied, &stop, &next, &keep);
            workers.push(scope.spawn(move || {
                // SAFETY: memory-mapped files are safe when files aren't modified during search
                let mut searcher = SearcherBuilder::new()
                    .memory_map(unsafe { MmapChoice::auto() })
                    .line_number(true)
                    .build();
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(index) else {
                        break;
                    };
                    let mut hits = Vec::new();
                    if prefilter.is_some_and(|filter| !filter.may_contain(path)) {
                        if tx.send((index, hits)).is_err() {
                            break;
                        }
                        continue;
                    }
                    // Lines past a pattern's `limit` in one file can never be
                    // taken. A pattern already satisfied by earlier files is
                    // skipped too: every file before this one had been
                    // searched when that was decided.
                    let mut taken = vec![0usize; exact.len()];
                    // A separate search would abort the whole file at the
                    // first non-UTF-8 line it matched; this marks the patterns
                    // that did.
                    let mut abandoned = vec![false; exact.len()];
                    search_source_file(
                        &mut searcher,
                        matcher,
                        path,
                        Bytes(|line_num, bytes| {
                            let line = bytes.strip_suffix(b"\n").unwrap_or(bytes);
                            let text = std::str::from_utf8(bytes).ok();
                            let mut open = false;
                            for index in 0..exact.len() {
                                if abandoned[index]
                                    || taken[index] >= limit
                                    || satisfied[index].load(Ordering::Relaxed)
                                {
                                    continue;
                                }
                                open = true;
                                if !required[index].is_match(line)
                                    || !exact[index].is_match(line).unwrap_or(false)
                                {
                                    continue;
                                }
                                let Some(text) = text else {
                                    abandoned[index] = true;
                                    continue;
                                };
                                let text = text.trim_end();
                                if !keep(index, path, text) {
                                    continue;
                                }
                                taken[index] += 1;
                                hits.push(PatternHit {
                                    pattern: index,
                                    line_num: line_num as usize,
                                    text: text.to_string(),
                                });
                            }
                            Ok(open && !stop.load(Ordering::Relaxed))
                        }),
                    );
                    if tx.send((index, hits)).is_err() {
                        break;
                    }
                }
            }));
        }
        drop(tx);

        let mut taken = vec![0usize; patterns.len()];
        let mut open = patterns.len();
        let mut pending = HashMap::new();
        let mut frontier = 0usize;
        'files: for (index, hits) in &rx {
            pending.insert(index, hits);
            while let Some(hits) = pending.remove(&frontier) {
                let path = &files[frontier];
                frontier += 1;
                for hit in hits {
                    if taken[hit.pattern] == limit {
                        continue;
                    }
                    handler(hit.pattern, path, hit.line_num, &hit.text);
                    taken[hit.pattern] += 1;
                    if taken[hit.pattern] == limit {
                        satisfied[hit.pattern].store(true, Ordering::Relaxed);
                        open -= 1;
                    }
                }
                if open == 0 {
                    stop.store(true, Ordering::Relaxed);
                    break 'files;
                }
            }
        }
        drop(rx);
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("parallel file search worker panicked"))?;
        }
        Ok(())
    })
}

/// Parallel walker over the primary root with the ignore rules the indexer
/// applies, or none when the index was built with `--no-ignore`.
fn project_walker(root: &Path) -> Result<ignore::WalkParallel> {
    let no_ignore = try_is_no_ignore_enabled(root)?;
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
    Ok(wb.build_parallel())
}
