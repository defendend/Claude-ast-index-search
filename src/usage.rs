//! Local usage log: one row per CLI invocation, read back by `ast-index usage`.
//!
//! Recording hooks into `main` once, around the whole dispatch, so every
//! subcommand — including ones added later — is logged without touching its
//! `cmd_*` function. The command name comes from clap's matched subcommand
//! chain, and output size is measured at the file-descriptor level (see
//! [`tee`]) because `println!` cannot be intercepted in-process.
//!
//! The log is local only: nothing is sent anywhere. `AST_INDEX_NO_USAGE=1`
//! disables it. Recording is best-effort — any failure to open or write the
//! log is swallowed, never surfaced to the command being run.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db;

/// MCP sets this to `mcp` on the `ast-index` processes it spawns. Any other
/// wrapper may set its own label.
pub const CALLER_ENV: &str = "AST_INDEX_CALLER";
/// Correlates an MCP tool call with the CLI row it produced, so the MCP
/// server can attach the size of the compacted response it returned.
pub const USAGE_ID_ENV: &str = "AST_INDEX_USAGE_ID";
pub const MCP_TOOL_ENV: &str = "AST_INDEX_MCP_TOOL";
pub const DISABLE_ENV: &str = "AST_INDEX_NO_USAGE";

const RETENTION: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const MAX_ARG_CHARS: usize = 200;
const MAX_ARGS: usize = 32;

/// Commands that are not logged: the report itself (and the MCP response
/// hook it hosts) and the long-running watcher, whose duration and output
/// say nothing about a query.
const UNRECORDED_COMMANDS: &[&str] = &["usage", "watch"];

static ACTIVE: Mutex<Option<Session>> = Mutex::new(None);

struct Session {
    started: Instant,
    ts_ms: i64,
    command: String,
    args: Vec<String>,
    source: String,
    mcp_tool: Option<String>,
    usage_id: Option<String>,
    project_root: Option<PathBuf>,
    tee: Option<tee::StdoutTee>,
}

/// Held by `main` for the whole run. Dropping it without [`Guard::finish`]
/// (a panic unwinding out of the dispatch) records the run as failed.
pub struct Guard(());

impl Guard {
    pub fn finish(self, exit_code: i32) {
        finish_active(exit_code);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        finish_active(101);
    }
}

/// Begin recording this process's invocation. Parses the real argv against
/// `cli` to learn the subcommand; an argv clap rejects (including `--help`)
/// is not recorded, since clap exits before any command runs.
pub fn start(cli: clap::Command) -> Guard {
    if let Some(session) = begin_session(cli) {
        if let Ok(mut active) = ACTIVE.lock() {
            *active = Some(session);
        }
    }
    Guard(())
}

/// `AST_INDEX_NO_USAGE=1` (any value other than empty, `0` or `false`).
pub fn recording_disabled() -> bool {
    env_flag(DISABLE_ENV)
}

fn begin_session(cli: clap::Command) -> Option<Session> {
    if recording_disabled() {
        return None;
    }
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let matches = cli.try_get_matches_from(&argv).ok()?;
    let chain = subcommand_chain(&matches);
    let first = chain.first()?;
    if UNRECORDED_COMMANDS.contains(&first.as_str()) || is_internal_worker(&matches) {
        return None;
    }
    db::usage_db_path()?;

    Some(Session {
        started: Instant::now(),
        ts_ms: now_ms(),
        command: chain.join(" "),
        args: truncate_args(argv.iter().skip(1).map(|a| a.to_string_lossy().into_owned())),
        source: detect_source(),
        mcp_tool: env_value(MCP_TOOL_ENV),
        usage_id: env_value(USAGE_ID_ENV),
        project_root: None,
        tee: tee::StdoutTee::start(),
    })
}

/// Attach the project root `main` resolved, so the report can scope rows to
/// the project it is run from.
pub fn set_project_root(root: &Path) {
    if let Ok(mut active) = ACTIVE.lock() {
        if let Some(session) = active.as_mut() {
            session.project_root = Some(root.to_path_buf());
        }
    }
}

/// Write the active invocation's row. Also called directly right before a
/// `std::process::exit`, which would otherwise skip the guard's drop and lose
/// buffered output still inside the tee.
pub fn finish_active(exit_code: i32) {
    let session = match ACTIVE.lock() {
        Ok(mut active) => active.take(),
        Err(_) => None,
    };
    let Some(mut session) = session else {
        return;
    };
    let output = session.tee.take().map(tee::StdoutTee::finish);
    let row = NewInvocation {
        ts_ms: session.ts_ms,
        project_root: project_key(
            session
                .project_root
                .or_else(|| std::env::current_dir().ok())
                .as_deref(),
        ),
        command: session.command,
        args: serde_json::to_string(&session.args).unwrap_or_else(|_| "[]".into()),
        source: session.source,
        mcp_tool: session.mcp_tool,
        usage_id: session.usage_id,
        duration_ms: session.started.elapsed().as_millis() as i64,
        exit_code,
        output,
    };
    let Some(path) = db::usage_db_path() else {
        return;
    };
    let _ = open_store(&path).and_then(|conn| insert_invocation(&conn, &row));
}

/// Record the size of the response the MCP server actually returned (after
/// its compaction) against the CLI row carrying `usage_id`.
pub fn record_response(usage_id: &str, response_bytes: u64) -> Result<()> {
    let Some(path) = db::usage_db_path().filter(|p| p.exists()) else {
        return Ok(());
    };
    let conn = open_store(&path)?;
    conn.execute(
        "UPDATE invocations SET response_bytes = ?1 WHERE usage_id = ?2",
        params![response_bytes as i64, usage_id],
    )?;
    Ok(())
}

fn subcommand_chain(matches: &clap::ArgMatches) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = matches;
    while let Some((name, sub)) = current.subcommand() {
        chain.push(name.to_string());
        current = sub;
    }
    chain
}

/// The background update worker is spawned by another ast-index process; its
/// run is already accounted for by the command that queued it.
fn is_internal_worker(matches: &clap::ArgMatches) -> bool {
    let Some((_, sub)) = matches.subcommand() else {
        return false;
    };
    ["coordinator_worker", "coordinator_launch"].iter().any(|flag| {
        sub.try_get_one::<bool>(flag)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false)
    })
}

fn detect_source() -> String {
    if let Some(caller) = env_value(CALLER_ENV) {
        return caller;
    }
    if env_flag("CLAUDECODE") {
        return "claude-code".into();
    }
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        "human".into()
    } else {
        "script".into()
    }
}

fn truncate_args(args: impl Iterator<Item = String>) -> Vec<String> {
    args.take(MAX_ARGS)
        .map(|arg| {
            if arg.chars().count() > MAX_ARG_CHARS {
                let mut cut: String = arg.chars().take(MAX_ARG_CHARS).collect();
                cut.push('…');
                cut
            } else {
                arg
            }
        })
        .collect()
}

/// Rows are keyed by the canonical project root so a symlinked checkout and
/// its target report together.
pub fn project_key(root: Option<&Path>) -> String {
    match root {
        Some(root) => std::fs::canonicalize(root)
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .into_owned(),
        None => String::new(),
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_flag(name: &str) -> bool {
    env_value(name)
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS invocations (
        id INTEGER PRIMARY KEY,
        ts_ms INTEGER NOT NULL,
        project_root TEXT NOT NULL,
        command TEXT NOT NULL,
        args TEXT NOT NULL,
        source TEXT NOT NULL,
        mcp_tool TEXT,
        usage_id TEXT,
        version TEXT NOT NULL,
        duration_ms INTEGER NOT NULL,
        exit_code INTEGER NOT NULL,
        stdout_bytes INTEGER,
        stdout_lines INTEGER,
        response_bytes INTEGER,
        no_results INTEGER NOT NULL DEFAULT 0,
        index_missing INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_invocations_project_ts ON invocations(project_root, ts_ms);
    CREATE INDEX IF NOT EXISTS idx_invocations_usage_id ON invocations(usage_id);
";

/// Opens (creating if needed) the log file, but never its directory: commands
/// such as `version` or `changed` must not create the cache layout, so the
/// log only starts once something else — an index build — has created it.
pub fn open_store(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            anyhow::bail!("cache directory {} does not exist", parent.display());
        }
    }
    let conn = Connection::open(path)?;
    init_store(&conn)?;
    Ok(conn)
}

/// A store with the log's schema and no rows, for reporting before anything
/// has been recorded without creating files.
pub fn empty_store() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn init_store(conn: &Connection) -> Result<()> {
    // Concurrent agents run many short invocations at once; wait briefly for
    // the writer lock rather than dropping the row.
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

struct NewInvocation {
    ts_ms: i64,
    project_root: String,
    command: String,
    args: String,
    source: String,
    mcp_tool: Option<String>,
    usage_id: Option<String>,
    duration_ms: i64,
    exit_code: i32,
    output: Option<tee::OutputStats>,
}

fn insert_invocation(conn: &Connection, row: &NewInvocation) -> Result<()> {
    let output = row.output.as_ref();
    conn.execute(
        "INSERT INTO invocations (ts_ms, project_root, command, args, source, mcp_tool, usage_id,
             version, duration_ms, exit_code, stdout_bytes, stdout_lines, no_results, index_missing)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            row.ts_ms,
            row.project_root,
            row.command,
            row.args,
            row.source,
            row.mcp_tool,
            row.usage_id,
            env!("CARGO_PKG_VERSION"),
            row.duration_ms,
            row.exit_code,
            output.map(|o| o.bytes as i64),
            output.map(|o| o.lines as i64),
            output.map(|o| o.no_results).unwrap_or(false),
            output.map(|o| o.index_missing).unwrap_or(false),
        ],
    )?;
    // Prune occasionally instead of on every write; the log only needs to be
    // bounded, not exact.
    if conn.last_insert_rowid() % 256 == 0 {
        conn.execute(
            "DELETE FROM invocations WHERE ts_ms < ?1",
            params![row.ts_ms - RETENTION.as_millis() as i64],
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Invocation {
    pub ts_ms: i64,
    pub project_root: String,
    pub command: String,
    pub source: String,
    pub mcp_tool: Option<String>,
    pub duration_ms: i64,
    pub exit_code: i32,
    pub stdout_bytes: Option<i64>,
    pub stdout_lines: Option<i64>,
    pub response_bytes: Option<i64>,
    pub no_results: bool,
    pub index_missing: bool,
}

/// Rows at or after `since_ms`, optionally limited to one project.
pub fn query_invocations(
    conn: &Connection,
    project_root: Option<&str>,
    since_ms: i64,
) -> Result<Vec<Invocation>> {
    let mut stmt = conn.prepare(
        "SELECT ts_ms, project_root, command, source, mcp_tool, duration_ms, exit_code,
                stdout_bytes, stdout_lines, response_bytes, no_results, index_missing
         FROM invocations
         WHERE ts_ms >= ?1 AND (?2 IS NULL OR project_root = ?2)
         ORDER BY ts_ms",
    )?;
    let rows = stmt.query_map(params![since_ms, project_root], |r| {
        Ok(Invocation {
            ts_ms: r.get(0)?,
            project_root: r.get(1)?,
            command: r.get(2)?,
            source: r.get(3)?,
            mcp_tool: r.get(4)?,
            duration_ms: r.get(5)?,
            exit_code: r.get(6)?,
            stdout_bytes: r.get(7)?,
            stdout_lines: r.get(8)?,
            response_bytes: r.get(9)?,
            no_results: r.get(10)?,
            index_missing: r.get(11)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Count of rows at or after `since_ms` and the timestamp of the oldest one.
pub fn count_since(
    conn: &Connection,
    project_root: Option<&str>,
    since_ms: i64,
) -> Result<(i64, Option<i64>)> {
    let row = conn
        .query_row(
            "SELECT COUNT(*), MIN(ts_ms) FROM invocations
             WHERE ts_ms >= ?1 AND (?2 IS NULL OR project_root = ?2)",
            params![since_ms, project_root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.unwrap_or((0, None)))
}

// ---------------------------------------------------------------------------
// Output scanning
// ---------------------------------------------------------------------------

/// Only the start of each line is inspected; the markers we look for are
/// short prefixes, and holding whole lines of a large result is wasteful.
const LINE_PREFIX: usize = 256;
/// JSON output is kept whole (up to this size) so emptiness can be judged
/// from its structure rather than from line shapes.
const JSON_CAP: usize = 4 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutputStats {
    pub bytes: u64,
    pub lines: u64,
    pub no_results: bool,
    pub index_missing: bool,
}

/// Incremental scanner over the bytes a command writes to stdout.
#[derive(Default)]
pub struct OutputScanner {
    stats: OutputStats,
    line: Vec<u8>,
    /// `None` until the first non-whitespace byte decides whether the output
    /// is JSON; then `Some(buffer)` for JSON, `Some` cleared + `json = false`
    /// otherwise.
    json: Option<bool>,
    json_buf: Vec<u8>,
}

impl OutputScanner {
    pub fn feed(&mut self, chunk: &[u8]) {
        self.stats.bytes += chunk.len() as u64;
        if self.json.is_none() {
            if let Some(&first) = chunk.iter().find(|b| !b.is_ascii_whitespace()) {
                self.json = Some(first == b'{' || first == b'[');
            }
        }
        if self.json == Some(true) {
            if self.json_buf.len() + chunk.len() <= JSON_CAP {
                self.json_buf.extend_from_slice(chunk);
            } else {
                self.json = Some(false);
                self.json_buf = Vec::new();
            }
        }
        for &byte in chunk {
            if byte == b'\n' {
                self.stats.lines += 1;
                self.inspect_line();
                self.line.clear();
            } else if self.line.len() < LINE_PREFIX {
                self.line.push(byte);
            }
        }
    }

    pub fn finish(mut self) -> OutputStats {
        if !self.line.is_empty() {
            self.stats.lines += 1;
            self.inspect_line();
        }
        if self.json == Some(true) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&self.json_buf) {
                self.stats.no_results = is_empty_json_result(&value);
            }
        }
        self.stats
    }

    fn inspect_line(&mut self) {
        let text = strip_ansi(&String::from_utf8_lossy(&self.line));
        let text = text.trim();
        if text.contains("Index not found") {
            self.stats.index_missing = true;
        }
        if is_empty_result_line(text) {
            self.stats.no_results = true;
        }
    }
}

/// Text-mode shapes commands use for an empty result: "No usages found.",
/// "Callers of 'X' (showing 0 of 0):", "explore: no symbols matched 'X'".
fn is_empty_result_line(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    (text.starts_with("no ") && text.contains(" found"))
        || text.contains("(showing 0 of 0)")
        || (text.contains(": no ") && (text.contains(" found") || text.contains(" matched")))
}

/// JSON results are empty when every result list in them is: a bare `[]`, or
/// an object whose array fields (`items`, `symbols`, `files`, …) all are.
fn is_empty_json_result(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.is_empty(),
        serde_json::Value::Object(fields) => {
            let mut arrays = fields.values().filter_map(|v| v.as_array()).peekable();
            arrays.peek().is_some() && arrays.all(|a| a.is_empty())
        }
        _ => false,
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Stdout tee
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod tee {
    //! Measures what the process writes to fd 1 by swapping it for a pipe
    //! whose reader forwards every byte to the original stdout.
    //!
    //! Only engaged when stdout is not a terminal, so interactive output keeps
    //! its tty (colour, width) and piped output — the agent and MCP case —
    //! sees a pipe either way. Child processes inherit the pipe; their output
    //! is counted too, which matches what the caller receives.

    use std::io::Write;
    use std::os::fd::RawFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    pub use super::OutputStats;
    use super::OutputScanner;

    pub struct StdoutTee {
        original: RawFd,
        stop: Arc<AtomicBool>,
        reader: Option<JoinHandle<OutputStats>>,
    }

    impl StdoutTee {
        pub fn start() -> Option<Self> {
            if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                return None;
            }
            // SAFETY: plain fd syscalls on descriptors this function owns; on
            // every failure path the descriptors opened so far are closed and
            // fd 1 is left as it was.
            unsafe {
                let mut fds = [0 as RawFd; 2];
                if libc::pipe(fds.as_mut_ptr()) != 0 {
                    return None;
                }
                let (read_end, write_end) = (fds[0], fds[1]);
                libc::fcntl(read_end, libc::F_SETFD, libc::FD_CLOEXEC);
                libc::fcntl(write_end, libc::F_SETFD, libc::FD_CLOEXEC);
                let original = libc::fcntl(1, libc::F_DUPFD_CLOEXEC, 3);
                if original < 0 {
                    libc::close(read_end);
                    libc::close(write_end);
                    return None;
                }
                let _ = std::io::stdout().flush();
                if libc::dup2(write_end, 1) < 0 {
                    libc::close(read_end);
                    libc::close(write_end);
                    libc::close(original);
                    return None;
                }
                libc::close(write_end);
                let flags = libc::fcntl(read_end, libc::F_GETFL);
                libc::fcntl(read_end, libc::F_SETFL, flags | libc::O_NONBLOCK);

                let stop = Arc::new(AtomicBool::new(false));
                let reader_stop = Arc::clone(&stop);
                let reader = std::thread::Builder::new()
                    .name("usage-stdout-tee".into())
                    .spawn(move || pump(read_end, original, &reader_stop));
                match reader {
                    Ok(reader) => Some(Self {
                        original,
                        stop,
                        reader: Some(reader),
                    }),
                    Err(_) => {
                        libc::dup2(original, 1);
                        libc::close(original);
                        None
                    }
                }
            }
        }

        pub fn finish(mut self) -> OutputStats {
            let _ = std::io::stdout().flush();
            // SAFETY: `original` is a descriptor this tee owns. Restoring it
            // onto fd 1 closes the pipe's write end held there, which lets the
            // reader see EOF once it has drained everything written so far.
            unsafe {
                libc::dup2(self.original, 1);
            }
            self.stop.store(true, Ordering::SeqCst);
            let stats = self
                .reader
                .take()
                .and_then(|reader| reader.join().ok())
                .unwrap_or_default();
            // SAFETY: the reader has exited, so nothing else uses `original`.
            unsafe {
                libc::close(self.original);
            }
            stats
        }
    }

    fn pump(read_end: RawFd, out: RawFd, stop: &AtomicBool) -> OutputStats {
        let mut scanner = OutputScanner::default();
        let mut buf = vec![0u8; 64 * 1024];
        'outer: loop {
            let mut pfd = libc::pollfd {
                fd: read_end,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pfd` is a valid pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut pfd, 1, 50) };
            if ready < 0 {
                if last_errno() == libc::EINTR {
                    continue;
                }
                break;
            }
            if ready == 0 {
                // A detached child may still hold the write end, so EOF is not
                // guaranteed; once `finish` has restored fd 1 an empty pipe
                // means everything this process wrote has been forwarded.
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            loop {
                // SAFETY: `buf` is valid for `buf.len()` bytes.
                let n = unsafe { libc::read(read_end, buf.as_mut_ptr().cast(), buf.len()) };
                if n > 0 {
                    let chunk = &buf[..n as usize];
                    scanner.feed(chunk);
                    if !write_all(out, chunk) {
                        // Downstream closed (`| head`). Closing our end makes
                        // the command's next write fail as it would have
                        // without the tee.
                        break 'outer;
                    }
                } else if n == 0 {
                    break 'outer;
                } else {
                    match last_errno() {
                        libc::EINTR => continue,
                        libc::EAGAIN => break,
                        _ => break 'outer,
                    }
                }
            }
        }
        // SAFETY: the read end is owned by this thread.
        unsafe {
            libc::close(read_end);
        }
        scanner.finish()
    }

    fn write_all(fd: RawFd, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            // SAFETY: `data` is valid for `data.len()` bytes.
            let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
            if n > 0 {
                data = &data[n as usize..];
            } else if n < 0 && last_errno() == libc::EINTR {
                continue;
            } else {
                return false;
            }
        }
        true
    }

    fn last_errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
}

#[cfg(not(unix))]
mod tee {
    pub use super::OutputStats;

    pub struct StdoutTee;

    impl StdoutTee {
        pub fn start() -> Option<Self> {
            None
        }

        pub fn finish(self) -> OutputStats {
            OutputStats::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&str]) -> OutputStats {
        let mut scanner = OutputScanner::default();
        for chunk in chunks {
            scanner.feed(chunk.as_bytes());
        }
        scanner.finish()
    }

    #[test]
    fn scanner_counts_bytes_and_lines_across_chunks() {
        let stats = scan(&["a\nb", "c\n", "tail"]);
        assert_eq!(stats.bytes, 9);
        assert_eq!(stats.lines, 3);
        assert!(!stats.no_results);
    }

    #[test]
    fn scanner_flags_empty_results_through_ansi() {
        assert!(scan(&["Usages of Foo:\n  \u{1b}[2mNo usages found.\u{1b}[0m\n"]).no_results);
        assert!(scan(&["Callers of 'Foo' (showing 0 of 0):\n"]).no_results);
        assert!(scan(&["explore: no symbols matched 'Foo'\n"]).no_results);
        assert!(!scan(&["Nothing to see\n"]).no_results);
    }

    #[test]
    fn scanner_judges_json_by_structure() {
        assert!(scan(&["[]\n"]).no_results);
        assert!(scan(&["{\"items\": [],", " \"pagination\": {\"total\": 0}}\n"]).no_results);
        assert!(scan(&["{\"files\": [], \"symbols\": []}"]).no_results);
        assert!(!scan(&["{\"files\": [], \"symbols\": [{\"name\": \"Foo\"}]}"]).no_results);
        assert!(!scan(&["{\"file_count\": 0}"]).no_results);
        // A text line that looks empty inside a JSON payload is data, not a verdict.
        assert!(!scan(&["{\"items\": [\"No usages found\"]}"]).no_results);
    }

    #[test]
    fn scanner_flags_missing_index() {
        let stats = scan(&["\u{1b}[31mIndex not found. Run 'ast-index rebuild' first.\u{1b}[0m\n"]);
        assert!(stats.index_missing);
    }

    #[test]
    fn subcommand_chain_follows_nested_subcommands() {
        let cli = clap::Command::new("ast-index")
            .subcommand(clap::Command::new("graph").subcommand(clap::Command::new("build")));
        let matches = cli.try_get_matches_from(["ast-index", "graph", "build"]).unwrap();
        assert_eq!(subcommand_chain(&matches), vec!["graph", "build"]);
    }

    #[test]
    fn internal_worker_flag_is_detected() {
        let cli = clap::Command::new("ast-index").subcommand(
            clap::Command::new("update").arg(
                clap::Arg::new("coordinator_worker")
                    .long("coordinator-worker")
                    .action(clap::ArgAction::SetTrue),
            ),
        );
        let worker = cli
            .clone()
            .try_get_matches_from(["ast-index", "update", "--coordinator-worker"])
            .unwrap();
        let plain = cli.try_get_matches_from(["ast-index", "update"]).unwrap();
        assert!(is_internal_worker(&worker));
        assert!(!is_internal_worker(&plain));
    }

    #[test]
    fn long_args_are_truncated() {
        let args = truncate_args(vec!["x".repeat(500)].into_iter());
        assert_eq!(args[0].chars().count(), MAX_ARG_CHARS + 1);
    }
}
