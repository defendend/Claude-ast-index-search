//! Per-file VCS history signals and the `hotspots` report built on top.
//!
//! Two code paths live here. The collector reads `git log --numstat` in
//! bounded windows into a per-commit store (what each commit did to each
//! path) and folds the commits HEAD reaches into commit counts, churn, bugfix
//! ratio, author sets and timestamps in `git_file_stats` /
//! `git_file_authors`. The reporter reads those rows back and turns the raw
//! numbers into percentile ranks *within this repository*, so "high churn"
//! means high relative to its neighbours instead of relative to a constant
//! that only ever fits one repository size.
//!
//! Collection is never implicit: `rebuild` and `update` stay untouched and
//! the user opts in with `hotspots --collect`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use colored::Colorize;
use rayon::prelude::*;
use regex::Regex;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;

use super::changed::{
    discover_vcs_root, os_args, parse_utf8, render_stderr, run_bounded, Deadline, Vcs, STDOUT_LIMIT,
};
use super::graph::is_test_path;
use super::Page;
use crate::db::{self, GitFileSignalRow, GitFileStats};

/// Commit cursor: HEAD at the last successful collection.
const META_HEAD: &str = "git_signals_head";
/// Absolute VCS root the cursor belongs to.
const META_REPO_ROOT: &str = "git_signals_repo_root";
/// Project-root-relative pathspec the collection was scoped to.
const META_SCOPE: &str = "git_signals_scope";
/// Wall-clock time of the last successful collection, Unix milliseconds.
const META_COLLECTED_AT: &str = "git_signals_collected_at";
/// Commits HEAD reaches that changed the project: the analysed history.
const META_COMMITS: &str = "git_signals_commits";
/// Layout of the per-commit store the derived tables were folded from.
const META_STORE: &str = "git_signals_store";
const STORE_LAYOUT: &str = "commits-v1";
/// Paths that carry history once renames are followed, deleted ones included.
const META_PATHS: &str = "git_signals_paths";

/// Commits per `git rev-list` window when listing the commit graph; a line
/// is about 100 bytes, well under the captured-stdout ceiling.
const GRAPH_WINDOW: usize = 100_000;
/// Commits HEAD no longer reaches stay in the store, so switching back costs
/// nothing, until there are more than this many of them or more than one
/// per [`DEAD_COMMITS_SHARE`] live commits; then all of them are dropped.
const DEAD_COMMITS_FLOOR: usize = 1_000;
const DEAD_COMMITS_SHARE: usize = 4;

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

/// How a collection run reached the current HEAD.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectMode {
    /// Nothing reusable: first run, `--full`, or the stored history had to be
    /// discarded (see `reset_reason`).
    Full,
    /// The stored history was moved to HEAD: commits HEAD no longer reaches
    /// were subtracted, commits it newly reaches were added.
    Incremental,
}

#[derive(Clone, Debug, Serialize)]
pub struct CollectOutcome {
    pub mode: CollectMode,
    /// Commits whose diffs were read from Git in this run.
    pub commits_scanned: usize,
    /// Commits HEAD reaches again whose diffs came from the per-commit store
    /// (switching back to a branch collected before).
    pub commits_restored: usize,
    /// Commits that left the collected history (branch switch, rebase, reset).
    pub commits_dropped: usize,
    /// Paths whose history was recomputed.
    pub paths_touched: usize,
    pub head: String,
    pub previous_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_reason: Option<String>,
    pub elapsed_ms: u128,
}

/// A commit as `git rev-list --parents --timestamp` lists it.
struct GraphCommit {
    sha: String,
    committed_at: i64,
    parents: Vec<String>,
}

/// One commit of `git log --numstat`.
struct CommitRecord {
    sha: String,
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

/// A [`FileChange`] in project-relative terms, ready for the store.
struct ProjectChange {
    kind: i64,
    path: String,
    from: Option<String>,
    added: i64,
    deleted: i64,
}

struct NewCommit {
    sha: String,
    order_key: i64,
}

/// What a run changes in the per-commit store, worked out from Git before
/// the write transaction starts.
struct Plan {
    full: bool,
    /// The stored cursor the plan starts from; `None` for a full plan.
    base: Option<String>,
    new_commits: Vec<NewCommit>,
    records: Vec<CommitRecord>,
    restored: Vec<db::StoredGitCommit>,
    dropped: Vec<db::StoredGitCommit>,
}

struct Applied {
    paths_touched: usize,
    commits_restored: usize,
    commits_dropped: usize,
}

struct Collector {
    executable: OsString,
    repo_root: PathBuf,
    project_root: PathBuf,
    /// Project root relative to the repo root; `None` when they coincide.
    scope: Option<String>,
    deadline: Deadline,
    verbose: bool,
    window: usize,
}

impl Collector {
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

    /// Full hash of `revision` as a commit, `None` when it does not name one.
    fn resolve_commit(&self, revision: &str) -> Result<Option<String>> {
        let args = vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(format!("{revision}^{{commit}}")),
        ];
        let output = run_bounded(
            &self.executable,
            &args,
            &self.repo_root,
            self.deadline,
            self.verbose,
        )
        .with_context(|| format!("failed to resolve {revision}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let sha = parse_utf8(&output.stdout.bytes, "git rev-parse output")?
            .trim()
            .to_string();
        Ok((!sha.is_empty()).then_some(sha))
    }

    fn push_pathspec(&self, args: &mut Vec<OsString>) {
        if let Some(scope) = &self.scope {
            args.push(OsString::from("--"));
            args.push(OsString::from(scope.as_str()));
        }
    }

    /// Git's own history simplification would hide side-branch commits whose
    /// changes a merge discarded, and hide them differently depending on
    /// where the walk starts. Every commit that changes the project counts,
    /// so that a commit's contribution never depends on the range it was
    /// read in.
    fn push_history_mode(&self, args: &mut Vec<OsString>) {
        if self.scope.is_some() {
            args.push(OsString::from("--full-history"));
        }
    }

    fn count(&self, prefix: &[OsString], revs: &[OsString], pathspec: bool) -> Result<usize> {
        let mut args = prefix.to_vec();
        args.extend(revs.iter().cloned());
        if pathspec {
            self.push_pathspec(&mut args);
        }
        let bytes = self.git(&args)?;
        parse_utf8(&bytes, "git rev-list output")?
            .trim()
            .parse::<usize>()
            .context("git rev-list --count did not return a number")
    }

    /// Run `prefix … revs` over `total` commits in `--skip`/`--max-count`
    /// windows. A window whose output overflows is split in half.
    fn windowed<T>(
        &self,
        prefix: &[OsString],
        revs: &[OsString],
        pathspec: bool,
        total: usize,
        window: usize,
        parse: fn(&[u8]) -> Result<Vec<T>>,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut offset = 0;
        while offset < total {
            let count = window.max(1).min(total - offset);
            self.window_into(prefix, revs, pathspec, offset, count, parse, &mut out)?;
            offset += count;
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn window_into<T>(
        &self,
        prefix: &[OsString],
        revs: &[OsString],
        pathspec: bool,
        skip: usize,
        count: usize,
        parse: fn(&[u8]) -> Result<Vec<T>>,
        out: &mut Vec<T>,
    ) -> Result<()> {
        let mut args = prefix.to_vec();
        args.push(OsString::from(format!("--skip={skip}")));
        args.push(OsString::from(format!("--max-count={count}")));
        args.extend(revs.iter().cloned());
        if pathspec {
            self.push_pathspec(&mut args);
        }
        match self.git_allow_truncation(&args)? {
            Some(bytes) => {
                out.extend(parse(&bytes)?);
                Ok(())
            }
            None if count <= 1 => bail!(
                "a single commit's output exceeded {STDOUT_LIMIT} bytes; \
                 collection cannot proceed"
            ),
            None => {
                let first = count / 2;
                self.window_into(prefix, revs, pathspec, skip, first, parse, out)?;
                self.window_into(
                    prefix,
                    revs,
                    pathspec,
                    skip + first,
                    count - first,
                    parse,
                    out,
                )
            }
        }
    }

    /// Every commit `revs` selects, merges and commits outside the project
    /// included, with its parents: the graph the order keys are built from.
    fn list_graph(&self, revs: &[OsString]) -> Result<Vec<GraphCommit>> {
        let total = self.count(&os_args(&["rev-list", "--count"]), revs, false)?;
        self.windowed(
            &os_args(&["rev-list", "--parents", "--timestamp"]),
            revs,
            false,
            total,
            GRAPH_WINDOW,
            parse_graph,
        )
    }

    /// Diffstats of every non-merge commit `revs` selects that changes the
    /// project.
    fn read_numstat(&self, revs: &[OsString]) -> Result<Vec<CommitRecord>> {
        let mut count_args = os_args(&["rev-list", "--count", "--no-merges"]);
        self.push_history_mode(&mut count_args);
        let total = self.count(&count_args, revs, true)?;
        let mut log_args = os_args(&[
            "log",
            "--no-merges",
            "--numstat",
            "-z",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "--format=%x01%H%x1f%at%x1f%ae%x1f%an%x1f%s",
        ]);
        self.push_history_mode(&mut log_args);
        if self.verbose {
            eprintln!(
                "hotspots: reading diffs of {total} commit(s) in windows of {}",
                self.window.max(1)
            );
        }
        self.windowed(&log_args, revs, true, total, self.window, parse_git_log)
    }

    /// Plan for a store rebuilt from scratch at `head`.
    fn plan_full(&self, head: &str) -> Result<Plan> {
        let revs = vec![OsString::from(head)];
        let graph = self.list_graph(&revs)?;
        // A shallow clone lists parents it does not have; they are skipped.
        let keys = order_keys(&graph, |_| Ok(None), false)?
            .ok_or_else(|| anyhow!("commit graph has a parent loop"))?;
        let records = self.read_numstat(&revs)?;
        let new_commits = graph
            .into_iter()
            .map(|commit| NewCommit {
                order_key: keys[&commit.sha],
                sha: commit.sha,
            })
            .collect();
        Ok(Plan {
            full: true,
            base: None,
            new_commits,
            records,
            restored: Vec::new(),
            dropped: Vec::new(),
        })
    }

    /// Plan that moves the stored history from `base` to `head`: commits only
    /// `base` reaches leave it, commits only `head` reaches join it, read
    /// from Git unless the store still has them. `None` when the store does
    /// not match the repository and has to be rebuilt.
    fn plan_sync(&self, conn: &Connection, base: &str, head: &str) -> Result<Option<Plan>> {
        let mut plan = Plan {
            full: false,
            base: Some(base.to_string()),
            new_commits: Vec::new(),
            records: Vec::new(),
            restored: Vec::new(),
            dropped: Vec::new(),
        };
        if base == head {
            return Ok(Some(plan));
        }
        let arrived = self.list_graph(&os_args(&[head, "--not", base]))?;
        let departed = self.list_graph(&os_args(&[base, "--not", head]))?;

        for commit in departed {
            match db::find_git_commit(conn, &commit.sha)? {
                Some(stored) if stored.live => plan.dropped.push(stored),
                _ => return Ok(None),
            }
        }
        let mut unread = Vec::new();
        for commit in arrived {
            match db::find_git_commit(conn, &commit.sha)? {
                Some(stored) if !stored.live => plan.restored.push(stored),
                Some(_) => return Ok(None),
                None => unread.push(commit),
            }
        }

        // Every parent of an unread commit is either unread too or already
        // stored; the stored ones bound the range Git has to diff. Stored
        // commits are closed under "parent of", so `head --not <boundary>`
        // selects exactly the unread commits.
        let mut boundary: BTreeSet<String> = BTreeSet::new();
        boundary.insert(base.to_string());
        let keys = order_keys(
            &unread,
            |parent| {
                let stored = db::find_git_commit(conn, parent)?;
                if stored.is_some() {
                    boundary.insert(parent.to_string());
                }
                Ok(stored.map(|stored| stored.order_key))
            },
            true,
        )?;
        let Some(keys) = keys else {
            return Ok(None);
        };
        if unread.iter().any(|commit| commit.parents.len() <= 1) {
            let mut revs = os_args(&[head, "--not"]);
            revs.extend(boundary.iter().map(OsString::from));
            plan.records = self.read_numstat(&revs)?;
            let unread_shas: HashSet<&str> = unread.iter().map(|c| c.sha.as_str()).collect();
            if plan
                .records
                .iter()
                .any(|record| !unread_shas.contains(record.sha.as_str()))
            {
                return Ok(None);
            }
        }
        plan.new_commits = unread
            .into_iter()
            .map(|commit| NewCommit {
                order_key: keys[&commit.sha],
                sha: commit.sha,
            })
            .collect();
        Ok(Some(plan))
    }

    /// Translate a repo-relative change into project-relative terms, or drop
    /// it when it happens entirely outside the project.
    fn project_change(&self, change: &FileChange) -> Option<ProjectChange> {
        match change {
            FileChange::Touched {
                path,
                added,
                deleted,
            } => Some(ProjectChange {
                kind: db::GIT_CHANGE_TOUCH,
                path: self.project_relative(path)?,
                from: None,
                added: *added,
                deleted: *deleted,
            }),
            FileChange::Renamed {
                from,
                to,
                added,
                deleted,
            } => match (self.project_relative(from), self.project_relative(to)) {
                (Some(source), Some(target)) if source != target => Some(ProjectChange {
                    kind: db::GIT_CHANGE_RENAME,
                    path: target,
                    from: Some(source),
                    added: *added,
                    deleted: *deleted,
                }),
                (_, Some(target)) => Some(ProjectChange {
                    kind: db::GIT_CHANGE_TOUCH,
                    path: target,
                    from: None,
                    added: *added,
                    deleted: *deleted,
                }),
                (Some(source), None) => Some(ProjectChange {
                    kind: db::GIT_CHANGE_MOVED_OUT,
                    path: source,
                    from: None,
                    added: 0,
                    deleted: 0,
                }),
                (None, None) => None,
            },
        }
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
}

/// Corrected commit date of every commit in `pending`: its committer date,
/// raised to one past its latest parent's.
///
/// This is the order the history fold replays commits in. It is a property
/// of the commit alone (its date and ancestry), so a commit sits in the same
/// place whichever range it was read in, and it never puts a commit before
/// its parent even when clocks disagree. Parents outside `pending` come from
/// `known`; one it does not know makes the result `None` when `strict`, and
/// is skipped otherwise (a shallow clone's missing parents).
fn order_keys(
    pending: &[GraphCommit],
    mut known: impl FnMut(&str) -> Result<Option<i64>>,
    strict: bool,
) -> Result<Option<HashMap<String, i64>>> {
    let index: HashMap<&str, &GraphCommit> = pending
        .iter()
        .map(|commit| (commit.sha.as_str(), commit))
        .collect();
    let mut keys: HashMap<String, i64> = HashMap::with_capacity(pending.len());
    let mut outside: HashMap<String, Option<i64>> = HashMap::new();
    for start in pending {
        if keys.contains_key(&start.sha) {
            continue;
        }
        let mut stack: Vec<(&str, bool)> = vec![(start.sha.as_str(), false)];
        while let Some((sha, expanded)) = stack.pop() {
            if keys.contains_key(sha) {
                continue;
            }
            let commit = index[sha];
            if !expanded {
                stack.push((sha, true));
                for parent in &commit.parents {
                    if index.contains_key(parent.as_str()) && !keys.contains_key(parent) {
                        stack.push((parent.as_str(), false));
                    }
                }
                continue;
            }
            let mut key = commit.committed_at;
            for parent in &commit.parents {
                let parent_key = if index.contains_key(parent.as_str()) {
                    match keys.get(parent) {
                        Some(value) => Some(*value),
                        None => bail!("commit graph has a parent loop at {}", short_sha(sha)),
                    }
                } else {
                    if !outside.contains_key(parent) {
                        let value = known(parent)?;
                        outside.insert(parent.clone(), value);
                    }
                    outside[parent]
                };
                match parent_key {
                    Some(value) => key = key.max(value + 1),
                    None if strict => return Ok(None),
                    None => {}
                }
            }
            keys.insert(sha.to_string(), key);
        }
    }
    Ok(Some(keys))
}

/// Parse `git rev-list --parents --timestamp` output.
fn parse_graph(bytes: &[u8]) -> Result<Vec<GraphCommit>> {
    let text = parse_utf8(bytes, "git rev-list output")?;
    let mut commits = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let (Some(timestamp), Some(sha)) = (fields.next(), fields.next()) else {
            continue;
        };
        commits.push(GraphCommit {
            committed_at: timestamp
                .parse::<i64>()
                .with_context(|| format!("bad commit timestamp in '{line}'"))?,
            sha: sha.to_string(),
            parents: fields.map(str::to_string).collect(),
        });
    }
    Ok(commits)
}

// ---------------------------------------------------------------------------
// History fold
// ---------------------------------------------------------------------------

/// One stored change, as the fold replays it.
#[derive(Clone, Copy)]
enum FoldChange {
    Touch {
        path: i64,
        added: i64,
        deleted: i64,
    },
    Rename {
        from: i64,
        to: i64,
        added: i64,
        deleted: i64,
    },
    MovedOut {
        path: i64,
    },
}

struct FoldCommit<'a> {
    order_key: i64,
    sha: &'a str,
    timestamp: i64,
    author: u32,
    is_fix: bool,
    changes: Vec<FoldChange>,
}

/// Mutable accumulator for one path while the fold runs.
#[derive(Default)]
struct Accumulator {
    commits: i64,
    fix_commits: i64,
    lines_added: i64,
    lines_deleted: i64,
    first_commit_at: Option<i64>,
    last_commit_at: Option<i64>,
    authors: HashSet<u32>,
    dirty: bool,
}

impl Accumulator {
    fn record(&mut self, commit: &FoldCommit<'_>, added: i64, deleted: i64) {
        self.commits += 1;
        if commit.is_fix {
            self.fix_commits += 1;
        }
        self.lines_added += added;
        self.lines_deleted += deleted;
        self.first_commit_at = min_option(self.first_commit_at, Some(commit.timestamp));
        self.last_commit_at = max_option(self.last_commit_at, Some(commit.timestamp));
        self.authors.insert(commit.author);
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

/// Author strings interned to small ids for the fold.
#[derive(Default)]
struct Authors {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Authors {
    fn intern(&mut self, author: &str) -> u32 {
        if let Some(id) = self.ids.get(author) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.names.push(author.to_string());
        self.ids.insert(author.to_string(), id);
        id
    }
}

/// Group stored changes into the commits `member` admits.
fn fold_input<'a>(
    changes: &[db::StoredGitChange],
    details: &'a HashMap<i64, db::StoredGitCommitDetail>,
    authors: &mut Authors,
    member: impl Fn(i64, &db::StoredGitCommitDetail) -> bool,
) -> Result<Vec<FoldCommit<'a>>> {
    let mut grouped: HashMap<i64, Vec<FoldChange>> = HashMap::new();
    for change in changes {
        let fold = match change.kind {
            db::GIT_CHANGE_RENAME => FoldChange::Rename {
                from: change
                    .from_path_id
                    .ok_or_else(|| anyhow!("stored rename without a source path"))?,
                to: change.path_id,
                added: change.added,
                deleted: change.deleted,
            },
            db::GIT_CHANGE_MOVED_OUT => FoldChange::MovedOut {
                path: change.path_id,
            },
            _ => FoldChange::Touch {
                path: change.path_id,
                added: change.added,
                deleted: change.deleted,
            },
        };
        grouped.entry(change.commit_id).or_default().push(fold);
    }
    let mut commits = Vec::with_capacity(grouped.len());
    for (commit_id, changes) in grouped {
        let detail = details
            .get(&commit_id)
            .ok_or_else(|| anyhow!("stored change points at unknown commit {commit_id}"))?;
        if !member(commit_id, detail) {
            continue;
        }
        commits.push(FoldCommit {
            order_key: detail.order_key,
            sha: detail.sha.as_str(),
            timestamp: detail.authored_at,
            author: authors.intern(&detail.author),
            is_fix: detail.is_fix,
            changes,
        });
    }
    Ok(commits)
}

/// Replay commits oldest first and accumulate per-path history.
///
/// A rename hands everything accumulated under the old path to the new one
/// and leaves the old path blank, so a file later created at the old path
/// starts from zero. Within one commit the order of changes does not matter:
/// a rename's source is a deleted path and its target an added one, so no
/// path is touched twice.
fn fold_history(commits: &mut [FoldCommit<'_>]) -> HashMap<i64, Accumulator> {
    commits.sort_by(|left, right| {
        left.order_key
            .cmp(&right.order_key)
            .then_with(|| left.sha.cmp(right.sha))
    });
    let mut paths: HashMap<i64, Accumulator> = HashMap::new();
    for commit in commits.iter() {
        for change in &commit.changes {
            match *change {
                FoldChange::Touch {
                    path,
                    added,
                    deleted,
                } => paths
                    .entry(path)
                    .or_default()
                    .record(commit, added, deleted),
                FoldChange::Rename {
                    from,
                    to,
                    added,
                    deleted,
                } => {
                    let previous = paths
                        .insert(from, Accumulator::default())
                        .unwrap_or_default();
                    let target = paths.entry(to).or_default();
                    target.absorb(previous);
                    target.record(commit, added, deleted);
                }
                FoldChange::MovedOut { path } => {
                    paths.insert(path, Accumulator::default());
                }
            }
        }
    }
    paths.retain(|_, accumulator| accumulator.dirty);
    paths
}

/// Rows for the folded paths that exist in the working tree.
fn stats_rows(
    folded: HashMap<i64, Accumulator>,
    names: &HashMap<i64, String>,
    authors: &Authors,
    project_root: &Path,
) -> Result<Vec<GitFileStats>> {
    let mut folded: Vec<(&String, Accumulator)> = folded
        .into_iter()
        .map(|(path_id, accumulator)| {
            names
                .get(&path_id)
                .map(|path| (path, accumulator))
                .ok_or_else(|| anyhow!("stored change points at unknown path {path_id}"))
        })
        .collect::<Result<_>>()?;
    folded.sort_by(|left, right| left.0.cmp(right.0));
    // Line counting reads every surviving file; on a large tree that is most
    // of a full collection's post-processing, and it parallelises cleanly.
    let rows = folded
        .into_par_iter()
        .filter_map(|(path, accumulator)| {
            let current_lines = count_lines(&project_root.join(path))?;
            let mut author_names: Vec<String> = accumulator
                .authors
                .iter()
                .map(|id| authors.names[*id as usize].clone())
                .collect();
            author_names.sort();
            Some(GitFileStats {
                path: path.clone(),
                commits: accumulator.commits,
                fix_commits: accumulator.fix_commits,
                lines_added: accumulator.lines_added,
                lines_deleted: accumulator.lines_deleted,
                first_commit_at: accumulator.first_commit_at,
                last_commit_at: accumulator.last_commit_at,
                current_lines: Some(current_lines),
                authors: author_names,
            })
        })
        .collect();
    Ok(rows)
}

/// `seeds` plus every path connected to one of them by a rename.
///
/// History only ever moves along renames, so the paths outside this set keep
/// exactly the history they had: no commit that joined or left touched them,
/// and none of their renames did either.
fn rename_closure(seeds: &HashSet<i64>, renames: &[(i64, i64)]) -> Vec<i64> {
    let mut neighbours: HashMap<i64, Vec<i64>> = HashMap::new();
    for &(from, to) in renames {
        neighbours.entry(from).or_default().push(to);
        neighbours.entry(to).or_default().push(from);
    }
    let mut seen: HashSet<i64> = seeds.clone();
    let mut queue: Vec<i64> = seeds.iter().copied().collect();
    while let Some(path) = queue.pop() {
        if let Some(next) = neighbours.get(&path) {
            for &other in next {
                if seen.insert(other) {
                    queue.push(other);
                }
            }
        }
    }
    let mut closure: Vec<i64> = seen.into_iter().collect();
    closure.sort_unstable();
    closure
}

/// Write a plan into the store and bring the derived tables up to date, all
/// in one transaction.
fn apply_plan(
    conn: &mut Connection,
    collector: &Collector,
    plan: &Plan,
    head: &str,
    repo_root: &str,
    scope_key: &str,
) -> Result<Applied> {
    let started = Instant::now();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to start git signal write")?;
    if !plan.full && db::get_metadata_value(&tx, META_HEAD)? != plan.base {
        bail!("another 'hotspots --collect' changed the collected history meanwhile; run it again");
    }
    if plan.full {
        db::clear_git_signals(&tx)?;
    }

    let records: HashMap<&str, &CommitRecord> = plan
        .records
        .iter()
        .map(|record| (record.sha.as_str(), record))
        .collect();
    let mut arrived: HashSet<i64> = HashSet::new();
    let mut commit_ids: HashMap<&str, i64> = HashMap::with_capacity(plan.new_commits.len());
    for commit in &plan.new_commits {
        let meta = records
            .get(commit.sha.as_str())
            .map(|record| db::GitCommitMeta {
                authored_at: record.timestamp,
                author: record.author.as_str(),
                is_fix: record.is_fix,
            });
        let id = db::insert_git_commit(&tx, &commit.sha, commit.order_key, meta)?;
        commit_ids.insert(commit.sha.as_str(), id);
        arrived.insert(id);
    }

    let mut touched: HashSet<i64> = HashSet::new();
    let mut path_ids: HashMap<String, i64> = HashMap::new();
    let mut path_id = |tx: &Connection, path: &str| -> Result<i64> {
        if let Some(id) = path_ids.get(path) {
            return Ok(*id);
        }
        let id = db::git_path_id(tx, path)?;
        path_ids.insert(path.to_string(), id);
        Ok(id)
    };
    for record in &plan.records {
        let commit_id = commit_ids[record.sha.as_str()];
        for change in &record.files {
            let Some(change) = collector.project_change(change) else {
                continue;
            };
            let target = path_id(&tx, &change.path)?;
            let source = match &change.from {
                Some(from) => Some(path_id(&tx, from)?),
                None => None,
            };
            db::insert_git_change(
                &tx,
                commit_id,
                target,
                change.kind,
                source,
                change.added,
                change.deleted,
            )?;
            touched.insert(target);
            touched.extend(source);
        }
    }
    let mut departed: HashSet<i64> = HashSet::new();
    for commit in &plan.restored {
        db::set_git_commit_live(&tx, commit.id, true)?;
        touched.extend(db::git_commit_touched_paths(&tx, commit.id)?);
        arrived.insert(commit.id);
    }
    for commit in &plan.dropped {
        db::set_git_commit_live(&tx, commit.id, false)?;
        touched.extend(db::git_commit_touched_paths(&tx, commit.id)?);
        departed.insert(commit.id);
    }

    let stored_at = started.elapsed();
    let mut authors = Authors::default();
    let (rows, paths_in_history, paths_touched) = if plan.full {
        let changes = db::load_git_changes(&tx, None)?;
        let details = db::load_git_commit_details(&tx, None)?;
        let mut commits = fold_input(&changes, &details, &mut authors, |_, detail| detail.live)?;
        let folded = fold_history(&mut commits);
        let names = db::load_git_paths(&tx, None)?;
        let paths_in_history = folded.len();
        let rows = stats_rows(folded, &names, &authors, &collector.project_root)?;
        (rows, paths_in_history, paths_in_history)
    } else if touched.is_empty() {
        (Vec::new(), stored_paths_in_history(&tx)?, 0)
    } else {
        let closure = rename_closure(&touched, &db::load_live_git_renames(&tx)?);
        let changes = db::load_git_changes(&tx, Some(&closure))?;
        let mut involved: Vec<i64> = changes.iter().map(|change| change.commit_id).collect();
        involved.sort_unstable();
        involved.dedup();
        let details = db::load_git_commit_details(&tx, Some(&involved))?;
        // The closure is closed under renames before and after the move, so
        // folding it both ways yields exactly its old and its new rows.
        let mut before = fold_input(&changes, &details, &mut authors, |id, detail| {
            (detail.live && !arrived.contains(&id)) || departed.contains(&id)
        })?;
        let old_paths = fold_history(&mut before).len();
        let mut after = fold_input(&changes, &details, &mut authors, |_, detail| detail.live)?;
        let folded = fold_history(&mut after);
        let new_paths = folded.len();
        let names = db::load_git_paths(&tx, Some(&closure))?;
        let stale: Vec<&str> = closure
            .iter()
            .filter_map(|path_id| names.get(path_id).map(String::as_str))
            .collect();
        db::delete_git_file_stats(&tx, &stale)?;
        let rows = stats_rows(folded, &names, &authors, &collector.project_root)?;
        let paths_in_history =
            (stored_paths_in_history(&tx)? + new_paths).saturating_sub(old_paths);
        (rows, paths_in_history, closure.len())
    };
    db::write_git_file_stats(&tx, &rows)?;
    let folded_at = started.elapsed();

    db::set_metadata_value(&tx, META_HEAD, head)?;
    db::set_metadata_value(&tx, META_REPO_ROOT, repo_root)?;
    db::set_metadata_value(&tx, META_SCOPE, scope_key)?;
    db::set_metadata_value(&tx, META_STORE, STORE_LAYOUT)?;
    db::set_metadata_value(&tx, META_COLLECTED_AT, &unix_millis_now().to_string())?;
    db::set_metadata_value(
        &tx,
        META_COMMITS,
        &db::count_live_git_commits(&tx)?.to_string(),
    )?;
    db::set_metadata_value(&tx, META_PATHS, &paths_in_history.to_string())?;

    let (live, dead) = db::count_git_commits(&tx)?;
    if dead > dead_commits_limit(live) {
        let pruned = db::prune_dead_git_commits(&tx)?;
        if collector.verbose {
            eprintln!("hotspots: pruned {pruned} commit(s) HEAD no longer reaches from the store");
        }
    }
    tx.commit().context("failed to commit git signal write")?;
    if collector.verbose {
        eprintln!(
            "hotspots: store written in {}ms, history folded in {}ms, committed in {}ms",
            stored_at.as_millis(),
            (folded_at - stored_at).as_millis(),
            (started.elapsed() - folded_at).as_millis()
        );
    }

    let changed_project = |commits: &[db::StoredGitCommit]| {
        commits
            .iter()
            .filter(|commit| commit.changed_project)
            .count()
    };
    Ok(Applied {
        paths_touched,
        commits_restored: changed_project(&plan.restored),
        commits_dropped: changed_project(&plan.dropped),
    })
}

fn dead_commits_limit(live: usize) -> usize {
    if let Some(limit) = std::env::var("AST_INDEX_TEST_GIT_DEAD_COMMITS_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        return limit;
    }
    DEAD_COMMITS_FLOOR.max(live / DEAD_COMMITS_SHARE)
}

fn stored_paths_in_history(conn: &Connection) -> Result<usize> {
    Ok(db::get_metadata_value(conn, META_PATHS)?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0))
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
        sha: sha.to_string(),
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
///
/// The per-commit store is moved to HEAD by set difference: commits only the
/// stored cursor reaches are subtracted, commits only HEAD reaches are added
/// (read from Git unless the store still has them from an earlier visit), and
/// only the paths those commits touched, plus paths linked to them by
/// renames, are recomputed. The result is the same as a full collection at
/// HEAD, row for row.
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
    let repo_key = canonical_repo.to_string_lossy().into_owned();
    let scope_key = scope.clone().unwrap_or_default();

    let collector = Collector {
        executable: super::changed::vcs_executable(Vcs::Git),
        repo_root: canonical_repo.clone(),
        project_root: canonical_project,
        scope,
        deadline: Deadline::new(Duration::from_millis(timeout_ms)),
        verbose,
        window,
    };

    let Some(head) = collector.resolve_commit("HEAD")? else {
        bail!(
            "{} has no commits yet; nothing to collect",
            canonical_repo.display()
        );
    };

    let stored_head = db::get_metadata_value(conn, META_HEAD)?;
    let stored_root = db::get_metadata_value(conn, META_REPO_ROOT)?;
    let stored_scope = db::get_metadata_value(conn, META_SCOPE)?;
    let stored_layout = db::get_metadata_value(conn, META_STORE)?;

    // A reset the user asked for needs no explanation; only history we had to
    // throw away on our own does.
    let mut reset_reason = None;
    let base = match stored_head.as_deref() {
        _ if full => None,
        None => None,
        Some(previous) if stored_root.as_deref() != Some(repo_key.as_str()) => {
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
        Some(_) if stored_layout.as_deref() != Some(STORE_LAYOUT) => {
            reset_reason = Some(
                "history collected by an older version has no per-commit store; \
                 recollecting once"
                    .to_string(),
            );
            None
        }
        Some(previous) => match collector.resolve_commit(previous)? {
            Some(_) => Some(previous.to_string()),
            None => {
                reset_reason = Some(format!(
                    "stored cursor {} is no longer in the repository; recollecting from scratch",
                    short_sha(previous)
                ));
                None
            }
        },
    };

    let plan = match base.as_deref() {
        Some(base) => match collector.plan_sync(conn, base, &head)? {
            Some(plan) => plan,
            None => {
                reset_reason = Some(
                    "the stored history does not match the repository; recollecting from scratch"
                        .to_string(),
                );
                collector.plan_full(&head)?
            }
        },
        None => collector.plan_full(&head)?,
    };
    if let (Some(reason), true) = (reset_reason.as_deref(), verbose) {
        eprintln!("hotspots: {reason}");
    }
    if verbose {
        eprintln!(
            "hotspots: full={} new={} read={} restored={} dropped={} scope={}",
            plan.full,
            plan.new_commits.len(),
            plan.records.len(),
            plan.restored.len(),
            plan.dropped.len(),
            collector.scope.as_deref().unwrap_or(".")
        );
    }

    let applied = apply_plan(conn, &collector, &plan, &head, &repo_key, &scope_key)?;
    Ok(CollectOutcome {
        mode: if plan.full {
            CollectMode::Full
        } else {
            CollectMode::Incremental
        },
        commits_scanned: plan.records.len(),
        commits_restored: applied.commits_restored,
        commits_dropped: applied.commits_dropped,
        paths_touched: applied.paths_touched,
        head,
        previous_head: stored_head,
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
    /// `score_exact` rounded for display.
    pub score: u32,
    /// Mean of the unrounded commits, churn and fix-ratio percentiles: what
    /// `--sort score` and the ranking presets order by, so files that share a
    /// rounded score near the top still come out in a meaningful order.
    #[serde(serialize_with = "serialize_round3")]
    pub score_exact: f64,
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
    #[serde(skip)]
    pub age_pct_exact: f64,
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
    /// `--exclude-tests`: test files are left out of `items`, not out of the
    /// percentile population.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tests_excluded: bool,
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
fn hotspot_score(commits_pct: f64, churn_pct: f64, fix_ratio_pct: f64) -> f64 {
    (commits_pct + churn_pct + fix_ratio_pct) / 3.0
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
            let score_exact = hotspot_score(
                commits_pct[position],
                churn_pct[position],
                fix_ratio_pct[position],
            );
            let mut hotspot = Hotspot {
                score: score_exact.round() as u32,
                score_exact,
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
                age_pct_exact: age_pct[position],
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
    /// `idle_pct` unrounded, for scoring.
    pub idle_pct_exact: f64,
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
                    idle_pct_exact: idle,
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

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Precise values are kept for ordering and printed with three decimals.
pub(crate) fn serialize_round3<S: serde::Serializer>(
    value: &f64,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_f64(round3(*value))
}

pub(crate) fn serialize_round3_option<S: serde::Serializer>(
    value: &Option<f64>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_some(&round3(*value)),
        None => serializer.serialize_none(),
    }
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
            _ => right.score_exact.total_cmp(&left.score_exact),
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
    exclude_tests: bool,
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
    // Tables written before the per-commit store kept a row per deleted path
    // too, and no path count.
    let paths_in_history = db::get_metadata_value(&conn, META_PATHS)?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(rows.len());
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
    // `--path`, `--min-commits` and `--exclude-tests` are applied after
    // ranking, so narrowing the report never silently redefines what "high"
    // means: a file keeps its percentiles whether or not tests are listed.
    let mut hotspots = build_hotspots(rows, now_seconds);
    hotspots.retain(|hotspot| {
        hotspot.commits >= min_commits
            && path_filter
                .map(|prefix| hotspot.path.starts_with(prefix))
                .unwrap_or(true)
            && !(exclude_tests && is_test_path(&hotspot.path))
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
        tests_excluded: exclude_tests,
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
        let mut moved = Vec::new();
        if collection.commits_restored > 0 {
            moved.push(format!(
                "{} restored from the history store",
                collection.commits_restored
            ));
        }
        if collection.commits_dropped > 0 {
            moved.push(format!(
                "{} no longer reachable from HEAD",
                collection.commits_dropped
            ));
        }
        let moved = if moved.is_empty() {
            String::new()
        } else {
            format!(" ({})", moved.join(", "))
        };
        println!(
            "{}",
            format!(
                "Collected {} commit(s) [{:?}]{moved}, recomputed {} path(s) in {}ms.",
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
            "Git hotspots — {} live file(s) of {} with history, {} commit(s) analyzed, HEAD {}, sorted by {}{}:",
            report.files_with_history,
            report.paths_in_history,
            report.commits_analyzed,
            report.head.as_deref().map(short_sha).unwrap_or("?"),
            report.sort,
            if report.tests_excluded {
                ", test files left out"
            } else {
                ""
            }
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

    fn signal_row(path: &str, commits: i64, fix_commits: i64, churn: i64) -> GitFileSignalRow {
        GitFileSignalRow {
            path: path.to_string(),
            commits,
            fix_commits,
            lines_added: churn,
            current_lines: Some(100),
            ..GitFileSignalRow::default()
        }
    }

    #[test]
    fn score_order_uses_unrounded_percentiles() {
        let mut rows: Vec<GitFileSignalRow> = (1..=100)
            .map(|index| signal_row(&format!("filler{index}.rs"), index, 0, index * 10))
            .collect();
        // `steady.rs` leads on commits and fix ratio, `churny.rs` only on churn:
        // both round to the same score, and the churn tie-break alone would
        // put `churny.rs` first.
        rows.push(signal_row("steady.rs", 1000, 800, 5000));
        rows.push(signal_row("churny.rs", 900, 700, 6000));
        let mut hotspots = build_hotspots(rows, 0);
        sort_hotspots(&mut hotspots, "score");
        assert_eq!(hotspots[0].score, hotspots[1].score);
        assert!(hotspots[0].score_exact > hotspots[1].score_exact);
        assert_eq!(hotspots[0].path, "steady.rs");
        assert_eq!(hotspots[1].path, "churny.rs");
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
