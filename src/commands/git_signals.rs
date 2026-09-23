//! Per-file VCS history signals and the `hotspots` report built on top.
//!
//! Two code paths live here. The collector walks `git log --numstat` in
//! bounded windows and accumulates commit counts, churn, bugfix ratio,
//! author sets and timestamps into `git_file_stats` / `git_file_authors`.
//! The reporter reads those rows back and turns the raw numbers into
//! percentile ranks *within this repository*, so "high churn" means high
//! relative to its neighbours instead of relative to a constant that only
//! ever fits one repository size.
//!
//! Collection is never implicit: `rebuild` and `update` stay untouched and
//! the user opts in with `hotspots --collect`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use colored::Colorize;
use regex::Regex;
use rusqlite::Connection;
use serde::Serialize;

use super::changed::{
    discover_vcs_root, os_args, parse_utf8, render_stderr, run_bounded, Deadline, Vcs, STDOUT_LIMIT,
};
use super::Page;
use crate::db::{self, GitFileSignalRow, GitFileStats};

/// Commit cursor: the last commit whose diffs are already accumulated.
const META_HEAD: &str = "git_signals_head";
/// Absolute VCS root the cursor belongs to.
const META_REPO_ROOT: &str = "git_signals_repo_root";
/// Project-root-relative pathspec the collection was scoped to.
const META_SCOPE: &str = "git_signals_scope";
/// Wall-clock time of the last successful collection, Unix milliseconds.
const META_COLLECTED_AT: &str = "git_signals_collected_at";
/// Number of commits folded into the tables so far.
const META_COMMITS: &str = "git_signals_commits";

/// Files bigger than this are not line-counted; relative churn is skipped.
const MAX_LINE_COUNT_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const PERCENTILE_HIGH: f64 = 90.0;
pub(crate) const PERCENTILE_ELEVATED: f64 = 75.0;
/// Below this many commits a fix ratio is noise (1 of 1 is not "100% bugs").
const MIN_COMMITS_FOR_FIX_LABEL: i64 = 4;
const SECONDS_PER_DAY: f64 = 86_400.0;

// ---------------------------------------------------------------------------
// Bugfix heuristic
// ---------------------------------------------------------------------------

/// Leading tracker key or issue number: `[ABC-123] `, `ABC-123: `, `#42 `.
/// Stripped before the bugfix match so a repository whose keys happen to read
/// `BUG-1234` does not score every commit as a fix.
fn issue_prefix_regex() -> &'static Regex {
    static CELL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"(?i)^\s*(?:\[[^\]]{1,40}\]|\(#\d+\)|#\d+|[a-z][a-z0-9_]{1,15}-\d+)[\s:.,-]*")
            .expect("issue prefix regex must compile")
    })
}

/// Bugfix vocabulary, English and Russian, matched on word boundaries.
///
/// Word boundaries matter: a substring match on `fix` also fires on `prefix`
/// and `suffix`. Russian entries are stems with a `\w*` tail because the
/// language inflects (`исправить` / `исправлен` / `исправление`).
fn bugfix_regex() -> &'static Regex {
    static CELL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)\b(",
            // English
            r"fix|fixes|fixed|fixing|fixup|bugfix|hotfix|bug|bugs|buggy",
            r"|regression|regressions|crash|crashes|crashing|broken|breakage",
            r"|revert|reverts|reverted|oops|typo|typos|repair|repairs",
            r"|workaround|resolve|resolves|resolved|correct|corrects|corrected",
            r"|incorrect|failure|failures|failing|misbehav\w*",
            // Russian
            r"|фикс\w*|пофикс\w*|исправ\w*|поправ\w*|баг\w*|ошиб\w*",
            r"|почин\w*|слома\w*|ломает\w*|отвалил\w*|отвалива\w*",
            r"|паден\w*|падает|падают|краш\w*|регресс\w*|устран\w*",
            r")\b",
        ))
        .expect("bugfix regex must compile")
    })
}

/// True when a commit subject reads like a bugfix rather than a change.
pub fn is_bugfix_subject(subject: &str) -> bool {
    let stripped = issue_prefix_regex().replace(subject, "");
    let body = if stripped.trim().is_empty() {
        subject
    } else {
        stripped.as_ref()
    };
    bugfix_regex().is_match(body)
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// Why a collection run had to start from scratch instead of resuming.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectMode {
    /// No usable cursor: first run, `--full`, or the cursor was discarded.
    Full,
    /// The stored cursor is still an ancestor of HEAD; only new commits read.
    Incremental,
}

#[derive(Clone, Debug, Serialize)]
pub struct CollectOutcome {
    pub mode: CollectMode,
    pub commits_scanned: usize,
    pub paths_touched: usize,
    pub head: String,
    pub previous_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_reason: Option<String>,
    pub elapsed_ms: u128,
}

struct CommitRecord {
    timestamp: i64,
    author: String,
    is_fix: bool,
    files: Vec<FileChange>,
}

enum FileChange {
    Touched {
        path: String,
        added: i64,
        deleted: i64,
    },
    Renamed {
        from: String,
        to: String,
        added: i64,
        deleted: i64,
    },
}

/// Mutable accumulator for one path while a collection run is in flight.
#[derive(Default)]
struct Accumulator {
    commits: i64,
    fix_commits: i64,
    lines_added: i64,
    lines_deleted: i64,
    first_commit_at: Option<i64>,
    last_commit_at: Option<i64>,
    authors: HashSet<String>,
    dirty: bool,
}

impl Accumulator {
    fn from_stored(stats: GitFileStats) -> Self {
        Self {
            commits: stats.commits,
            fix_commits: stats.fix_commits,
            lines_added: stats.lines_added,
            lines_deleted: stats.lines_deleted,
            first_commit_at: stats.first_commit_at,
            last_commit_at: stats.last_commit_at,
            authors: stats.authors.into_iter().collect(),
            dirty: false,
        }
    }

    fn record(&mut self, commit: &CommitRecord, added: i64, deleted: i64) {
        self.commits += 1;
        if commit.is_fix {
            self.fix_commits += 1;
        }
        self.lines_added += added;
        self.lines_deleted += deleted;
        self.first_commit_at = Some(match self.first_commit_at {
            Some(existing) => existing.min(commit.timestamp),
            None => commit.timestamp,
        });
        self.last_commit_at = Some(match self.last_commit_at {
            Some(existing) => existing.max(commit.timestamp),
            None => commit.timestamp,
        });
        self.authors.insert(commit.author.clone());
        self.dirty = true;
    }

    /// Fold a renamed predecessor's history into this path.
    fn absorb(&mut self, other: Accumulator) {
        self.commits += other.commits;
        self.fix_commits += other.fix_commits;
        self.lines_added += other.lines_added;
        self.lines_deleted += other.lines_deleted;
        self.first_commit_at = min_option(self.first_commit_at, other.first_commit_at);
        self.last_commit_at = max_option(self.last_commit_at, other.last_commit_at);
        self.authors.extend(other.authors);
        self.dirty = true;
    }
}

fn min_option(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (value, None) | (None, value) => value,
    }
}

fn max_option(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (value, None) | (None, value) => value,
    }
}

struct Collector<'a> {
    conn: &'a mut Connection,
    executable: OsString,
    repo_root: PathBuf,
    project_root: PathBuf,
    /// Project root relative to the repo root; `None` when they coincide.
    scope: Option<String>,
    deadline: Deadline,
    verbose: bool,
    window: usize,
    accumulators: HashMap<String, Accumulator>,
    /// Paths removed by a rename; their rows must disappear from the DB.
    retired: HashSet<String>,
    commits_scanned: usize,
}

impl Collector<'_> {
    fn git(&self, args: &[OsString]) -> Result<Vec<u8>> {
        self.git_allow_truncation(args)?
            .ok_or_else(|| anyhow!("git output exceeded {STDOUT_LIMIT} bytes"))
    }

    /// Run git under the shared deadline and process-tree guard. A truncated
    /// stdout comes back as `None` rather than an error so the caller can
    /// retry with a smaller commit window.
    fn git_allow_truncation(&self, args: &[OsString]) -> Result<Option<Vec<u8>>> {
        let output = run_bounded(
            &self.executable,
            args,
            &self.repo_root,
            self.deadline,
            self.verbose,
        )
        .context("git command failed")?;
        if !output.status.success() {
            let stderr = render_stderr(&output.stderr);
            bail!(
                "git exited with {}{}",
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }
        if output.stdout.truncated {
            return Ok(None);
        }
        Ok(Some(output.stdout.bytes))
    }

    fn rev_parse_head(&self) -> Result<Option<String>> {
        let args = os_args(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
        let output = run_bounded(
            &self.executable,
            &args,
            &self.repo_root,
            self.deadline,
            self.verbose,
        )
        .context("failed to resolve HEAD")?;
        if !output.status.success() {
            return Ok(None);
        }
        let head = parse_utf8(&output.stdout.bytes, "git rev-parse output")?
            .trim()
            .to_string();
        Ok((!head.is_empty()).then_some(head))
    }

    /// `Ok(true)` when `commit` exists and is an ancestor of HEAD, i.e. the
    /// stored cursor still describes a prefix of the current history.
    fn is_ancestor_of_head(&self, commit: &str) -> Result<bool> {
        let args = vec![
            OsString::from("merge-base"),
            OsString::from("--is-ancestor"),
            OsString::from(commit),
            OsString::from("HEAD"),
        ];
        let output = run_bounded(
            &self.executable,
            &args,
            &self.repo_root,
            self.deadline,
            self.verbose,
        )
        .context("failed to compare stored cursor with HEAD")?;
        Ok(output.status.success())
    }

    fn count_commits(&self, range: &str) -> Result<usize> {
        let mut args = os_args(&["rev-list", "--count", "--no-merges", range]);
        self.push_pathspec(&mut args);
        let bytes = self.git(&args)?;
        let text = parse_utf8(&bytes, "git rev-list output")?;
        text.trim()
            .parse::<usize>()
            .context("git rev-list --count did not return a number")
    }

    fn push_pathspec(&self, args: &mut Vec<OsString>) {
        if let Some(scope) = &self.scope {
            args.push(OsString::from("--"));
            args.push(OsString::from(scope.as_str()));
        }
    }

    /// Walk `range` oldest-commit-first in `--skip`/`-n` windows.
    ///
    /// Oldest-first matters for renames: when `old => new` shows up we move
    /// everything accumulated under `old` onto `new`, and that only produces
    /// the right totals if the pre-rename commits were folded in already.
    /// `--skip` and `-n` are applied during traversal, before `--reverse`
    /// re-orders the output, so window `w` always covers the same commits.
    fn walk(&mut self, range: &str, total: usize) -> Result<()> {
        if total == 0 {
            return Ok(());
        }
        let window = self.window.max(1);
        let mut boundaries = Vec::new();
        let mut cursor = 0usize;
        while cursor < total {
            boundaries.push((cursor, window.min(total - cursor)));
            cursor += window;
        }
        // Highest `--skip` first: that window holds the oldest commits.
        for (offset, count) in boundaries.into_iter().rev() {
            self.walk_window(range, offset, count)?;
        }
        Ok(())
    }

    fn walk_window(&mut self, range: &str, skip: usize, count: usize) -> Result<()> {
        let mut args = os_args(&[
            "log",
            "--reverse",
            "--no-merges",
            "--numstat",
            "-z",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "--format=%x01%H%x1f%at%x1f%ae%x1f%an%x1f%s",
        ]);
        args.push(OsString::from(format!("--skip={skip}")));
        args.push(OsString::from(format!("-n{count}")));
        args.push(OsString::from(range));
        self.push_pathspec(&mut args);

        match self.git_allow_truncation(&args)? {
            Some(bytes) => {
                for commit in parse_git_log(&bytes)? {
                    self.apply(&commit);
                }
                Ok(())
            }
            None if count <= 1 => bail!(
                "a single commit's diffstat exceeded {STDOUT_LIMIT} bytes; \
                 collection cannot proceed"
            ),
            None => {
                // Older half first, then the newer one, preserving order.
                let newer = count / 2;
                let older = count - newer;
                self.walk_window(range, skip + newer, older)?;
                self.walk_window(range, skip, newer)
            }
        }
    }

    fn apply(&mut self, commit: &CommitRecord) {
        self.commits_scanned += 1;
        for change in &commit.files {
            match change {
                FileChange::Touched {
                    path,
                    added,
                    deleted,
                } => {
                    let Some(key) = self.project_relative(path) else {
                        continue;
                    };
                    self.entry(&key).record(commit, *added, *deleted);
                }
                FileChange::Renamed {
                    from,
                    to,
                    added,
                    deleted,
                } => {
                    let source = self.project_relative(from);
                    let Some(target) = self.project_relative(to) else {
                        // Renamed out of the project root: retire the source.
                        if let Some(source) = source {
                            self.retire(source);
                        }
                        continue;
                    };
                    if let Some(source) = source {
                        if source != target {
                            self.ensure_loaded(&source);
                            let previous = self.retire(source);
                            self.entry(&target).absorb(previous);
                        }
                    }
                    self.entry(&target).record(commit, *added, *deleted);
                }
            }
        }
    }

    /// Hand back everything accumulated under `path` and leave a blank slate
    /// in its place.
    ///
    /// The blank accumulator is what keeps a resurrected path honest: the
    /// stored row is only deleted once the run commits, so without it a later
    /// commit re-creating this path would load the pre-rename history from
    /// the database and count it a second time.
    fn retire(&mut self, path: String) -> Accumulator {
        let previous = self
            .accumulators
            .insert(path.clone(), Accumulator::default());
        self.retired.insert(path);
        previous.unwrap_or_default()
    }

    fn entry(&mut self, path: &str) -> &mut Accumulator {
        self.ensure_loaded(path);
        self.accumulators
            .get_mut(path)
            .expect("accumulator was just inserted")
    }

    fn ensure_loaded(&mut self, path: &str) {
        if self.accumulators.contains_key(path) {
            return;
        }
        let stored = db::load_git_file_stats(self.conn, path)
            .ok()
            .flatten()
            .map(Accumulator::from_stored)
            .unwrap_or_default();
        self.accumulators.insert(path.to_string(), stored);
    }

    /// Convert a repo-root-relative path into a project-root-relative one,
    /// dropping anything that lives outside the indexed project.
    fn project_relative(&self, repo_path: &str) -> Option<String> {
        match &self.scope {
            None => Some(repo_path.to_string()),
            Some(scope) => repo_path
                .strip_prefix(scope.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_string),
        }
    }

    fn flush(&mut self) -> Result<Vec<GitFileStats>> {
        let mut rows = Vec::new();
        let accumulators = std::mem::take(&mut self.accumulators);
        for (path, accumulator) in accumulators {
            if !accumulator.dirty {
                continue;
            }
            let current_lines = count_lines(&self.project_root.join(&path));
            let mut authors: Vec<String> = accumulator.authors.into_iter().collect();
            authors.sort();
            rows.push(GitFileStats {
                path,
                commits: accumulator.commits,
                fix_commits: accumulator.fix_commits,
                lines_added: accumulator.lines_added,
                lines_deleted: accumulator.lines_deleted,
                first_commit_at: accumulator.first_commit_at,
                last_commit_at: accumulator.last_commit_at,
                current_lines,
                authors,
            });
        }
        rows.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(rows)
    }
}

/// Count newlines in a working-tree file; `None` when it is gone or huge.
fn count_lines(path: &Path) -> Option<i64> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    if metadata.len() > MAX_LINE_COUNT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return Some(0);
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as i64;
    Some(if bytes.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    })
}

/// Parse `git log --numstat -z --format=%x01…` output.
///
/// The stream is a flat sequence of NUL-terminated records. A record that
/// starts with `\x01` opens a commit; everything until the next such record
/// is that commit's numstat. A pure or modifying rename emits three records:
/// `added\tdeleted\t`, then the old path, then the new one.
fn parse_git_log(bytes: &[u8]) -> Result<Vec<CommitRecord>> {
    let mut commits: Vec<CommitRecord> = Vec::new();
    let mut fields = bytes.split(|byte| *byte == 0).peekable();

    while let Some(raw) = fields.next() {
        let field = trim_record_newlines(raw);
        if field.is_empty() {
            continue;
        }
        if let Some(header) = parse_commit_header(field)? {
            commits.push(header);
            continue;
        }
        let Some(commit) = commits.last_mut() else {
            continue;
        };
        // Lossy on purpose: Git paths are arbitrary bytes, and one file with a
        // latin-1 name must not abort the collection for the whole repository.
        let text = String::from_utf8_lossy(field);
        let mut parts = text.splitn(3, '\t');
        let added = parse_stat(parts.next().unwrap_or(""));
        let deleted = parse_stat(parts.next().unwrap_or(""));
        let tail = parts.next().unwrap_or("");
        if tail.is_empty() {
            // Rename: the two following records carry old and new path.
            let from = fields.next().map(trim_record_newlines).unwrap_or_default();
            let to = fields.next().map(trim_record_newlines).unwrap_or_default();
            if from.is_empty() || to.is_empty() {
                continue;
            }
            commit.files.push(FileChange::Renamed {
                from: String::from_utf8_lossy(from).into_owned(),
                to: String::from_utf8_lossy(to).into_owned(),
                added,
                deleted,
            });
        } else {
            commit.files.push(FileChange::Touched {
                path: tail.to_string(),
                added,
                deleted,
            });
        }
    }
    Ok(commits)
}

fn trim_record_newlines(field: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < field.len() && (field[start] == b'\n' || field[start] == b'\r') {
        start += 1;
    }
    &field[start..]
}

/// `-` in a numstat column means a binary file: counted as a touch, not churn.
fn parse_stat(value: &str) -> i64 {
    value.trim().parse::<i64>().unwrap_or(0)
}

fn parse_commit_header(field: &[u8]) -> Result<Option<CommitRecord>> {
    if field.first() != Some(&0x01) {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&field[1..]);
    let mut parts = text.split('\u{1f}');
    let sha = parts.next().unwrap_or("");
    if sha.len() < 7 || !sha.chars().all(|character| character.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let timestamp = parts
        .next()
        .unwrap_or("")
        .trim()
        .parse::<i64>()
        .unwrap_or(0);
    let email = parts.next().unwrap_or("").trim().to_lowercase();
    let name = parts.next().unwrap_or("").trim().to_string();
    let subject = parts.next().unwrap_or("");
    let author = if email.is_empty() { name } else { email };
    Ok(Some(CommitRecord {
        timestamp,
        author,
        is_fix: is_bugfix_subject(subject),
        files: Vec::new(),
    }))
}

/// Project root expressed relative to the VCS root, `/`-separated.
fn scope_within_repo(project_root: &Path, repo_root: &Path) -> Result<Option<String>> {
    let relative = project_root.strip_prefix(repo_root).with_context(|| {
        format!(
            "project root {} is outside VCS root {}",
            project_root.display(),
            repo_root.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!("project root is not a plain subdirectory of the VCS root"),
        }
    }
    Ok(Some(parts.join("/")))
}

/// Collect (or refresh) git signals for `project_root`.
pub fn collect_git_signals(
    project_root: &Path,
    conn: &mut Connection,
    full: bool,
    timeout_ms: u64,
    window: usize,
    verbose: bool,
) -> Result<CollectOutcome> {
    let started = Instant::now();
    let vcs_root = discover_vcs_root(project_root)?;
    if vcs_root.vcs != Vcs::Git {
        bail!(
            "git signals need a Git working tree; found {} at {}",
            vcs_root.vcs.command_name(),
            vcs_root.path.display()
        );
    }
    let canonical_project = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let canonical_repo = vcs_root
        .path
        .canonicalize()
        .unwrap_or_else(|_| vcs_root.path.clone());
    let scope = scope_within_repo(&canonical_project, &canonical_repo)?;

    let mut collector = Collector {
        conn,
        executable: super::changed::vcs_executable(Vcs::Git),
        repo_root: canonical_repo.clone(),
        project_root: canonical_project,
        scope: scope.clone(),
        deadline: Deadline::new(Duration::from_millis(timeout_ms)),
        verbose,
        window,
        accumulators: HashMap::new(),
        retired: HashSet::new(),
        commits_scanned: 0,
    };

    let Some(head) = collector.rev_parse_head()? else {
        bail!(
            "{} has no commits yet; nothing to collect",
            canonical_repo.display()
        );
    };

    let stored_head = db::get_metadata_value(collector.conn, META_HEAD)?;
    let stored_root = db::get_metadata_value(collector.conn, META_REPO_ROOT)?;
    let stored_scope = db::get_metadata_value(collector.conn, META_SCOPE)?;
    let scope_key = scope.clone().unwrap_or_default();

    // A reset the user asked for needs no explanation; only a cursor we had to
    // throw away on our own does.
    let mut reset_reason = None;
    let resume_from = if full {
        None
    } else {
        match stored_head.as_deref() {
            None => None,
            Some(previous)
                if stored_root.as_deref() != Some(canonical_repo.to_string_lossy().as_ref()) =>
            {
                reset_reason = Some(format!(
                    "stored cursor {} belongs to a different working tree",
                    short_sha(previous)
                ));
                None
            }
            Some(_) if stored_scope.as_deref().unwrap_or("") != scope_key => {
                reset_reason = Some("collection scope changed".to_string());
                None
            }
            Some(previous) if previous == head => Some(previous.to_string()),
            Some(previous) => {
                if collector.is_ancestor_of_head(previous)? {
                    Some(previous.to_string())
                } else {
                    reset_reason = Some(format!(
                        "stored cursor {} is no longer an ancestor of HEAD \
                         (branch switch, rebase or force-push); recollecting from scratch",
                        short_sha(previous)
                    ));
                    None
                }
            }
        }
    };

    let mode = if resume_from.is_some() {
        CollectMode::Incremental
    } else {
        CollectMode::Full
    };
    if let (Some(reason), true) = (reset_reason.as_deref(), verbose) {
        eprintln!("hotspots: {reason}");
    }
    if mode == CollectMode::Full {
        db::clear_git_file_stats(collector.conn)?;
        db::delete_metadata_value(collector.conn, META_HEAD)?;
    }

    let range = match resume_from.as_deref() {
        Some(previous) => format!("{previous}..{head}"),
        None => head.clone(),
    };
    let total = collector.count_commits(&range)?;
    if verbose {
        eprintln!(
            "hotspots: mode={mode:?} range={range} commits={total} window={} scope={}",
            collector.window,
            scope.as_deref().unwrap_or(".")
        );
    }
    collector.walk(&range, total)?;

    let rows = collector.flush()?;
    let paths_touched = rows.len();
    let written: HashSet<&str> = rows.iter().map(|row| row.path.as_str()).collect();
    let retired: Vec<String> = collector
        .retired
        .iter()
        .filter(|path| !written.contains(path.as_str()))
        .cloned()
        .collect();
    db::store_git_file_stats(collector.conn, &rows)?;
    for path in &retired {
        db::delete_git_file_stats(collector.conn, path)?;
    }

    let previous_head = stored_head.clone();
    db::set_metadata_value(collector.conn, META_HEAD, &head)?;
    db::set_metadata_value(
        collector.conn,
        META_REPO_ROOT,
        canonical_repo.to_string_lossy().as_ref(),
    )?;
    db::set_metadata_value(collector.conn, META_SCOPE, &scope_key)?;
    db::set_metadata_value(
        collector.conn,
        META_COLLECTED_AT,
        &unix_millis_now().to_string(),
    )?;
    let previous_commits = db::get_metadata_value(collector.conn, META_COMMITS)?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let cumulative = if mode == CollectMode::Full {
        collector.commits_scanned
    } else {
        previous_commits + collector.commits_scanned
    };
    db::set_metadata_value(collector.conn, META_COMMITS, &cumulative.to_string())?;

    Ok(CollectOutcome {
        mode,
        commits_scanned: collector.commits_scanned,
        paths_touched,
        head,
        previous_head,
        reset_reason,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

pub(crate) fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn unix_millis_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Percentiles and reporting
// ---------------------------------------------------------------------------

/// Percentile rank of every value against the whole population, 0..100.
///
/// Midrank handling of ties keeps a metric where most files share a value
/// (a repository full of single-commit files) from pushing that value to
/// the 100th percentile.
fn percentile_ranks(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    values
        .iter()
        .map(|value| midrank_percentile(&sorted, *value))
        .collect()
}

/// Midrank percentile of `value` against an ascending `sorted` population,
/// 0..100. `value` need not be a member: a value below every member is 0.
pub(crate) fn midrank_percentile(sorted: &[f64], value: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let less = sorted.partition_point(|candidate| *candidate < value);
    let not_greater = sorted.partition_point(|candidate| *candidate <= value);
    let equal = not_greater - less;
    100.0 * (less as f64 + 0.5 * equal as f64) / sorted.len() as f64
}

#[derive(Clone, Debug, Serialize)]
pub struct Hotspot {
    pub path: String,
    pub score: u32,
    pub commits: i64,
    pub commits_pct: u32,
    pub fix_commits: i64,
    pub fix_ratio: f64,
    pub fix_ratio_pct: u32,
    pub lines_added: i64,
    pub lines_deleted: i64,
    pub churn: i64,
    pub churn_pct: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_churn: Option<f64>,
    pub relative_churn_pct: u32,
    pub authors: usize,
    pub authors_pct: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_lines: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_days: Option<f64>,
    pub age_pct: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_since_change: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_commit_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit_at: Option<i64>,
    pub labels: Vec<String>,
}

/// `page` is flattened so the document carries the same `schema_version`,
/// `items` and `pagination` shape as every other paginated command; the
/// report-level fields sit alongside them.
#[derive(Debug, Serialize)]
pub struct HotspotsReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<i64>,
    pub commits_analyzed: usize,
    /// Files that still exist and therefore form the percentile population.
    pub files_with_history: usize,
    /// Every path the collector ever saw, deleted ones included.
    pub paths_in_history: usize,
    pub sort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<CollectOutcome>,
    #[serde(flatten)]
    pub page: Page<Hotspot>,
}

/// Thresholds are percentile-based on purpose: 28 commits is a lot for a
/// small library and unremarkable in a monorepo, so the only honest
/// reference point is the rest of this repository.
fn labels_for(hotspot: &Hotspot) -> Vec<String> {
    let mut labels = Vec::new();
    let pct = f64::from;
    if pct(hotspot.churn_pct) >= PERCENTILE_HIGH {
        labels.push("churn:high".to_string());
    } else if pct(hotspot.churn_pct) >= PERCENTILE_ELEVATED {
        labels.push("churn:elevated".to_string());
    }
    if pct(hotspot.relative_churn_pct) >= PERCENTILE_HIGH && hotspot.current_lines.is_some() {
        labels.push("rewritten-often".to_string());
    }
    if hotspot.commits >= MIN_COMMITS_FOR_FIX_LABEL {
        if pct(hotspot.fix_ratio_pct) >= PERCENTILE_HIGH {
            labels.push("fixes:high".to_string());
        } else if pct(hotspot.fix_ratio_pct) >= PERCENTILE_ELEVATED {
            labels.push("fixes:elevated".to_string());
        }
    }
    if pct(hotspot.authors_pct) >= PERCENTILE_HIGH {
        labels.push("authors:many".to_string());
    }
    if pct(hotspot.age_pct) >= PERCENTILE_HIGH {
        labels.push("veteran".to_string());
    }
    labels
}

/// Blend of the three signals an agent picking a file to copy cares about:
/// how often it moves, how often those moves are repairs, and how much text
/// the repairs rewrite. Reported as a 0..100 percentile blend, not an
/// absolute unit, for the same reason the labels are percentile-based.
fn hotspot_score(commits_pct: f64, churn_pct: f64, fix_ratio_pct: f64) -> u32 {
    ((commits_pct + churn_pct + fix_ratio_pct) / 3.0).round() as u32
}

fn build_hotspots(rows: Vec<GitFileSignalRow>, now_seconds: i64) -> Vec<Hotspot> {
    let commits: Vec<f64> = rows.iter().map(|row| row.commits as f64).collect();
    let churn: Vec<f64> = rows
        .iter()
        .map(|row| (row.lines_added + row.lines_deleted) as f64)
        .collect();
    let relative_churn: Vec<f64> = rows
        .iter()
        .map(|row| match row.current_lines {
            Some(lines) if lines > 0 => (row.lines_added + row.lines_deleted) as f64 / lines as f64,
            _ => 0.0,
        })
        .collect();
    let fix_ratio: Vec<f64> = rows
        .iter()
        .map(|row| {
            if row.commits > 0 {
                row.fix_commits as f64 / row.commits as f64
            } else {
                0.0
            }
        })
        .collect();
    let authors: Vec<f64> = rows.iter().map(|row| row.authors as f64).collect();
    let age: Vec<f64> = rows
        .iter()
        .map(|row| match row.first_commit_at {
            Some(first) => (now_seconds - first).max(0) as f64 / SECONDS_PER_DAY,
            None => 0.0,
        })
        .collect();

    let commits_pct = percentile_ranks(&commits);
    let churn_pct = percentile_ranks(&churn);
    let relative_churn_pct = percentile_ranks(&relative_churn);
    let fix_ratio_pct = percentile_ranks(&fix_ratio);
    let authors_pct = percentile_ranks(&authors);
    let age_pct = percentile_ranks(&age);

    rows.into_iter()
        .enumerate()
        .map(|(position, row)| {
            let mut hotspot = Hotspot {
                score: hotspot_score(
                    commits_pct[position],
                    churn_pct[position],
                    fix_ratio_pct[position],
                ),
                commits: row.commits,
                commits_pct: commits_pct[position].round() as u32,
                fix_commits: row.fix_commits,
                fix_ratio: round2(fix_ratio[position]),
                fix_ratio_pct: fix_ratio_pct[position].round() as u32,
                lines_added: row.lines_added,
                lines_deleted: row.lines_deleted,
                churn: row.lines_added + row.lines_deleted,
                churn_pct: churn_pct[position].round() as u32,
                relative_churn: row
                    .current_lines
                    .filter(|lines| *lines > 0)
                    .map(|_| round2(relative_churn[position])),
                relative_churn_pct: relative_churn_pct[position].round() as u32,
                authors: row.authors,
                authors_pct: authors_pct[position].round() as u32,
                current_lines: row.current_lines,
                age_days: row.first_commit_at.map(|_| round1(age[position])),
                age_pct: age_pct[position].round() as u32,
                days_since_change: row
                    .last_commit_at
                    .map(|last| round1((now_seconds - last).max(0) as f64 / SECONDS_PER_DAY)),
                first_commit_at: row.first_commit_at,
                last_commit_at: row.last_commit_at,
                path: row.path,
                labels: Vec::new(),
            };
            hotspot.labels = labels_for(&hotspot);
            hotspot
        })
        .collect()
}

/// One live file's history, ranked against every other live file exactly as
/// the `hotspots` report ranks it.
#[derive(Clone, Debug)]
pub struct FileHistory {
    pub hotspot: Hotspot,
    /// Percentile of days since the last change: high means untouched for
    /// longer than most files.
    pub idle_pct: u32,
}

/// The collected history, keyed by primary-root-relative path.
#[derive(Clone, Debug)]
pub struct HistorySnapshot {
    pub head: Option<String>,
    pub collected_at: Option<i64>,
    pub commits_analyzed: usize,
    pub files: HashMap<String, FileHistory>,
}

pub enum HistoryAvailability {
    /// `hotspots --collect` never ran against this index.
    NotCollected,
    /// A collection ran, but no file that still exists has history.
    Empty,
    Ready(HistorySnapshot),
}

/// Load every live file's history with percentiles and labels, for callers
/// that rank something other than the `hotspots` report by it.
pub fn load_history_snapshot(conn: &Connection) -> Result<HistoryAvailability> {
    let head = db::get_metadata_value(conn, META_HEAD)?;
    if head.is_none() {
        return Ok(HistoryAvailability::NotCollected);
    }
    let rows = db::load_live_git_file_signals(conn)?;
    if rows.is_empty() {
        return Ok(HistoryAvailability::Empty);
    }
    let now_seconds = unix_millis_now() / 1000;
    let idle: Vec<f64> = rows
        .iter()
        .map(|row| match row.last_commit_at {
            Some(last) => (now_seconds - last).max(0) as f64,
            None => 0.0,
        })
        .collect();
    let idle_pct = percentile_ranks(&idle);
    let files = build_hotspots(rows, now_seconds)
        .into_iter()
        .zip(idle_pct)
        .map(|(hotspot, idle)| {
            (
                hotspot.path.clone(),
                FileHistory {
                    hotspot,
                    idle_pct: idle.round() as u32,
                },
            )
        })
        .collect();
    Ok(HistoryAvailability::Ready(HistorySnapshot {
        head,
        collected_at: db::get_metadata_value(conn, META_COLLECTED_AT)?
            .and_then(|value| value.parse::<i64>().ok()),
        commits_analyzed: db::get_metadata_value(conn, META_COMMITS)?
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0),
        files,
    }))
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

const SORT_KEYS: [&str; 7] = [
    "score",
    "commits",
    "churn",
    "relative-churn",
    "fixes",
    "authors",
    "recent",
];

fn sort_hotspots(hotspots: &mut [Hotspot], sort: &str) {
    let compare = |left: &Hotspot, right: &Hotspot| -> std::cmp::Ordering {
        let ordering = match sort {
            "commits" => right.commits.cmp(&left.commits),
            "churn" => right.churn.cmp(&left.churn),
            "relative-churn" => right
                .relative_churn
                .unwrap_or(0.0)
                .partial_cmp(&left.relative_churn.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal),
            "fixes" => right
                .fix_ratio
                .partial_cmp(&left.fix_ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.fix_commits.cmp(&left.fix_commits)),
            "authors" => right.authors.cmp(&left.authors),
            "recent" => right.last_commit_at.cmp(&left.last_commit_at),
            _ => right.score.cmp(&left.score),
        };
        ordering
            .then_with(|| right.churn.cmp(&left.churn))
            .then_with(|| left.path.cmp(&right.path))
    };
    hotspots.sort_by(compare);
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_hotspots(
    root: &Path,
    collect: bool,
    full: bool,
    limit: usize,
    min_commits: i64,
    path_filter: Option<&str>,
    sort: &str,
    timeout_ms: u64,
    window: usize,
    verbose: bool,
    format: &str,
) -> Result<()> {
    if !SORT_KEYS.contains(&sort) {
        bail!("--sort must be one of: {}", SORT_KEYS.join(", "));
    }
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let mut conn = db::open_db(root)?;
    let collection = if collect || full {
        Some(collect_git_signals(
            root, &mut conn, full, timeout_ms, window, verbose,
        )?)
    } else {
        None
    };

    let rows = db::load_all_git_file_stats(&conn)?;
    let head = db::get_metadata_value(&conn, META_HEAD)?;
    let collected_at = db::get_metadata_value(&conn, META_COLLECTED_AT)?
        .and_then(|value| value.parse::<i64>().ok());
    let commits_analyzed = db::get_metadata_value(&conn, META_COMMITS)?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);

    if rows.is_empty() && format != "json" {
        if head.is_some() {
            println!("No git signals matched. The collected history is empty.");
        } else {
            println!(
                "{}",
                "No git signals collected yet. Run 'ast-index hotspots --collect'.".yellow()
            );
        }
        return Ok(());
    }

    // Paths are printed as stored, without `PathResolver`: git signals only
    // ever describe the primary root's own working tree (`--subtree` is
    // rejected up front), and probing extra roots could resolve a path onto a
    // same-named file in a different repository.
    let paths_in_history = rows.len();
    // Percentiles describe the files a reader can actually choose between, so
    // paths that no longer exist are dropped before ranking: a repository that
    // deleted half its history would otherwise inflate every survivor.
    let rows: Vec<GitFileSignalRow> = rows
        .into_iter()
        .filter(|row| row.current_lines.is_some())
        .map(GitFileSignalRow::from)
        .collect();
    let files_with_history = rows.len();
    let now_seconds = unix_millis_now() / 1000;
    // `--path` and `--min-commits` are applied after ranking, so narrowing the
    // report never silently redefines what "high" means.
    let mut hotspots = build_hotspots(rows, now_seconds);
    hotspots.retain(|hotspot| {
        hotspot.commits >= min_commits
            && path_filter
                .map(|prefix| hotspot.path.starts_with(prefix))
                .unwrap_or(true)
    });
    sort_hotspots(&mut hotspots, sort);

    let total = hotspots.len();
    let page = Page::new(hotspots, total, limit);
    let report = HotspotsReport {
        head: head.clone(),
        collected_at,
        commits_analyzed,
        files_with_history,
        paths_in_history,
        sort: sort.to_string(),
        collection,
        page,
    };

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    render_text(&report);
    Ok(())
}

fn render_text(report: &HotspotsReport) {
    if let Some(collection) = &report.collection {
        if let Some(reason) = &collection.reset_reason {
            println!("  {}", format!("Full recollect: {reason}").yellow());
        }
        println!(
            "{}",
            format!(
                "Collected {} commit(s) [{:?}] touching {} file(s) in {}ms.",
                collection.commits_scanned,
                collection.mode,
                collection.paths_touched,
                collection.elapsed_ms
            )
            .dimmed()
        );
    }
    println!(
        "{}",
        format!(
            "Git hotspots — {} live file(s) of {} with history, {} commit(s) analyzed, HEAD {}, sorted by {}:",
            report.files_with_history,
            report.paths_in_history,
            report.commits_analyzed,
            report.head.as_deref().map(short_sha).unwrap_or("?"),
            report.sort
        )
        .bold()
    );
    println!(
        "  {}",
        format!(
            "Labels are percentiles within this repository (high = p{}+, elevated = p{}+).",
            PERCENTILE_HIGH as u32, PERCENTILE_ELEVATED as u32
        )
        .dimmed()
    );

    for hotspot in &report.page.items {
        println!("  {}", hotspot.path.cyan());
        let relative = hotspot
            .relative_churn
            .map(|value| format!("{value:.1}x file"))
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "    score {} · commits {} (p{}) · fixes {}/{} = {:.0}% (p{}) · churn +{}/-{} (p{}, {})",
            hotspot.score,
            hotspot.commits,
            hotspot.commits_pct,
            hotspot.fix_commits,
            hotspot.commits,
            hotspot.fix_ratio * 100.0,
            hotspot.fix_ratio_pct,
            hotspot.lines_added,
            hotspot.lines_deleted,
            hotspot.churn_pct,
            relative,
        );
        println!(
            "    authors {} (p{}) · age {} · last change {} · {} lines",
            hotspot.authors,
            hotspot.authors_pct,
            hotspot
                .age_days
                .map(|days| format!("{days:.0}d"))
                .unwrap_or_else(|| "?".to_string()),
            hotspot
                .days_since_change
                .map(|days| format!("{days:.0}d ago"))
                .unwrap_or_else(|| "?".to_string()),
            hotspot
                .current_lines
                .map(|lines| lines.to_string())
                .unwrap_or_else(|| "?".to_string()),
        );
        if !hotspot.labels.is_empty() {
            println!("    {}", hotspot.labels.join(" ").yellow());
        }
    }

    if report.page.items.is_empty() {
        println!("  No files matched the filters.");
    }
    super::print_truncation_notice(report.page.pagination);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bugfix_heuristic_ignores_fix_inside_words() {
        assert!(!is_bugfix_subject("Add prefix handling to the parser"));
        assert!(!is_bugfix_subject("Refactor suffix array builder"));
        assert!(is_bugfix_subject("Fix suffix array builder"));
    }

    #[test]
    fn bugfix_heuristic_strips_tracker_prefix() {
        assert!(!is_bugfix_subject("[BUG-1234] Add a new export format"));
        assert!(is_bugfix_subject("[PTK-36985] Поправить тексты ошибок"));
        assert!(!is_bugfix_subject("[PTK-36985] Переименовать интеграцию"));
    }

    #[test]
    fn bugfix_heuristic_matches_russian_stems() {
        assert!(is_bugfix_subject("Исправление падения при импорте"));
        assert!(is_bugfix_subject("пофиксил баг в выгрузке"));
        assert!(!is_bugfix_subject("Добавить новую вкладку"));
    }

    #[test]
    fn percentile_ranks_use_midranks_for_ties() {
        let ranks = percentile_ranks(&[1.0, 1.0, 1.0, 10.0]);
        assert_eq!(ranks[3].round() as u32, 88);
        assert_eq!(ranks[0].round() as u32, 38);
    }

    #[test]
    fn percentile_ranks_handle_empty_input() {
        assert!(percentile_ranks(&[]).is_empty());
    }

    #[test]
    fn parse_git_log_reads_header_touch_and_rename() {
        let mut stream = Vec::new();
        stream.push(0x01);
        stream.extend_from_slice(
            "abcdef1234567890abcdef1234567890abcdef12\u{1f}1700000000\u{1f}dev@example.invalid\u{1f}Dev\u{1f}Fix crash".as_bytes(),
        );
        stream.push(0);
        stream.extend_from_slice(b"\n3\t1\tsrc/a.rs");
        stream.push(0);
        stream.extend_from_slice(b"0\t0\t");
        stream.push(0);
        stream.extend_from_slice(b"old/name.rs");
        stream.push(0);
        stream.extend_from_slice(b"new/name.rs");
        stream.push(0);

        let commits = parse_git_log(&stream).unwrap();
        assert_eq!(commits.len(), 1);
        assert!(commits[0].is_fix);
        assert_eq!(commits[0].author, "dev@example.invalid");
        assert_eq!(commits[0].files.len(), 2);
        match &commits[0].files[1] {
            FileChange::Renamed { from, to, .. } => {
                assert_eq!(from, "old/name.rs");
                assert_eq!(to, "new/name.rs");
            }
            _ => panic!("expected a rename record"),
        }
    }

    #[test]
    fn parse_git_log_treats_binary_columns_as_zero_churn() {
        let mut stream = Vec::new();
        stream.push(0x01);
        stream.extend_from_slice(
            "abcdef1234567890abcdef1234567890abcdef12\u{1f}1700000000\u{1f}d@e\u{1f}D\u{1f}Add blob"
                .as_bytes(),
        );
        stream.push(0);
        stream.extend_from_slice(b"\n-\t-\tassets/blob.bin");
        stream.push(0);

        let commits = parse_git_log(&stream).unwrap();
        match &commits[0].files[0] {
            FileChange::Touched {
                path,
                added,
                deleted,
            } => {
                assert_eq!(path, "assets/blob.bin");
                assert_eq!(*added, 0);
                assert_eq!(*deleted, 0);
            }
            _ => panic!("expected a touch record"),
        }
    }

    #[test]
    fn scope_within_repo_returns_none_for_identical_roots() {
        let root = Path::new("/tmp/project");
        assert_eq!(scope_within_repo(root, root).unwrap(), None);
        assert_eq!(
            scope_within_repo(Path::new("/tmp/project/sub/dir"), root).unwrap(),
            Some("sub/dir".to_string())
        );
    }
}
