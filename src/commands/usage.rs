//! `ast-index usage` — report the local usage log (see `crate::usage`).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};
use colored::Colorize;
use serde::Serialize;

use crate::db;
use crate::usage::{self, Invocation};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Days(i64),
    All,
}

impl Window {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("all") {
            return Ok(Window::All);
        }
        let days = value.strip_suffix('d').unwrap_or(value);
        match days.parse::<i64>() {
            Ok(days) if days > 0 => Ok(Window::Days(days)),
            _ => bail!("invalid --since '{value}': expected e.g. 7d, 30d or all"),
        }
    }

    fn since_ms(self, now_ms: i64) -> i64 {
        match self {
            Window::Days(days) => now_ms - days * DAY_MS,
            Window::All => 0,
        }
    }

    fn label(self) -> String {
        match self {
            Window::Days(days) => format!("{days}d"),
            Window::All => "all".into(),
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct CommandStats {
    command: String,
    calls: u64,
    errors: u64,
    no_results: u64,
    index_missing: u64,
    p50_ms: i64,
    p95_ms: i64,
    /// Calls whose stdout was measured (non-terminal stdout only).
    measured_calls: u64,
    stdout_bytes: u64,
    /// Bytes the MCP server returned after compacting stdout.
    mcp_response_bytes: u64,
    by_source: BTreeMap<String, u64>,
    #[serde(skip)]
    durations: Vec<i64>,
}

#[derive(Debug, Serialize)]
struct WindowCounts {
    last_7d: i64,
    last_30d: i64,
    all: i64,
    first_seen_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct Report {
    project: Option<String>,
    since: String,
    windows: WindowCounts,
    calls: u64,
    errors: u64,
    no_results: u64,
    index_missing: u64,
    stdout_bytes: u64,
    mcp_response_bytes: u64,
    by_source: BTreeMap<String, u64>,
    commands: Vec<CommandStats>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    projects: Vec<ProjectCount>,
}

#[derive(Debug, Serialize)]
struct ProjectCount {
    project: String,
    calls: u64,
}

pub fn cmd_usage(root: &Path, since: &str, all_projects: bool, format: &str) -> Result<()> {
    let window = Window::parse(since)?;
    let Some(path) = db::usage_db_path() else {
        bail!("cannot locate the ast-index cache directory");
    };
    if usage::recording_disabled() {
        eprintln!(
            "{}",
            format!("Note: {} is set, new calls are not being recorded.", usage::DISABLE_ENV).yellow()
        );
    }
    let conn = if path.exists() {
        usage::open_store(&path)?
    } else {
        usage::empty_store()?
    };
    let project = (!all_projects).then(|| usage::project_key(Some(root)));
    let now = usage::now_ms();

    let count = |since_ms| usage::count_since(&conn, project.as_deref(), since_ms);
    let (last_7d, _) = count(Window::Days(7).since_ms(now))?;
    let (last_30d, _) = count(Window::Days(30).since_ms(now))?;
    let (all, first_seen_ms) = count(0)?;
    let rows = usage::query_invocations(&conn, project.as_deref(), window.since_ms(now))?;

    let report = build_report(
        project,
        window,
        WindowCounts {
            last_7d,
            last_30d,
            all,
            first_seen_ms,
        },
        &rows,
        all_projects,
    );

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    print_report(&report);
    Ok(())
}

fn build_report(
    project: Option<String>,
    window: Window,
    windows: WindowCounts,
    rows: &[Invocation],
    all_projects: bool,
) -> Report {
    let mut commands: BTreeMap<String, CommandStats> = BTreeMap::new();
    let mut projects: BTreeMap<String, u64> = BTreeMap::new();
    let mut report = Report {
        project,
        since: window.label(),
        windows,
        calls: 0,
        errors: 0,
        no_results: 0,
        index_missing: 0,
        stdout_bytes: 0,
        mcp_response_bytes: 0,
        by_source: BTreeMap::new(),
        commands: Vec::new(),
        projects: Vec::new(),
    };

    for row in rows {
        let stats = commands.entry(row.command.clone()).or_insert_with(|| CommandStats {
            command: row.command.clone(),
            ..Default::default()
        });
        stats.calls += 1;
        stats.errors += u64::from(row.exit_code != 0);
        stats.no_results += u64::from(row.no_results);
        stats.index_missing += u64::from(row.index_missing);
        stats.durations.push(row.duration_ms);
        if let Some(bytes) = row.stdout_bytes {
            stats.measured_calls += 1;
            stats.stdout_bytes += bytes.max(0) as u64;
        }
        stats.mcp_response_bytes += row.response_bytes.unwrap_or(0).max(0) as u64;
        *stats.by_source.entry(row.source.clone()).or_default() += 1;
        *report.by_source.entry(row.source.clone()).or_default() += 1;
        if all_projects {
            *projects.entry(row.project_root.clone()).or_default() += 1;
        }
    }

    for stats in commands.values_mut() {
        stats.durations.sort_unstable();
        stats.p50_ms = percentile(&stats.durations, 50);
        stats.p95_ms = percentile(&stats.durations, 95);
        report.calls += stats.calls;
        report.errors += stats.errors;
        report.no_results += stats.no_results;
        report.index_missing += stats.index_missing;
        report.stdout_bytes += stats.stdout_bytes;
        report.mcp_response_bytes += stats.mcp_response_bytes;
    }
    report.commands = commands.into_values().collect();
    report
        .commands
        .sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.command.cmp(&b.command)));
    report.projects = projects
        .into_iter()
        .map(|(project, calls)| ProjectCount { project, calls })
        .collect();
    report
        .projects
        .sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.project.cmp(&b.project)));
    report
}

/// Nearest-rank percentile over an ascending slice.
fn percentile(sorted: &[i64], pct: usize) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

fn print_report(report: &Report) {
    let scope = report.project.as_deref().unwrap_or("all projects");
    println!("{}", format!("ast-index usage — {scope}").bold());
    let first_seen = report
        .windows
        .first_seen_ms
        .map(|ms| format!(" (since {})", format_date(ms)))
        .unwrap_or_default();
    println!(
        "  Calls: {} last 7 days, {} last 30 days, {} all time{}",
        report.windows.last_7d, report.windows.last_30d, report.windows.all, first_seen
    );

    if report.calls == 0 {
        println!("\n  No calls recorded in the last {}.", report.since);
        return;
    }

    let sources = report
        .by_source
        .iter()
        .map(|(source, calls)| format!("{source} {calls}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("\n{}", format!("Window {} — {} calls ({sources})", report.since, report.calls).bold());
    println!(
        "  {:<22} {:>6} {:>6} {:>7} {:>7} {:>7} {:>10} {:>10}",
        "Command", "Calls", "Errors", "Empty", "p50 ms", "p95 ms", "Stdout", "MCP sent"
    );
    for stats in &report.commands {
        println!(
            "  {} {:>6} {:>6} {:>7} {:>7} {:>7} {:>10} {:>10}",
            format!("{:<22}", stats.command).cyan(),
            stats.calls,
            stats.errors,
            stats.no_results,
            stats.p50_ms,
            stats.p95_ms,
            format_bytes(stats.stdout_bytes, stats.measured_calls),
            format_bytes(stats.mcp_response_bytes, u64::from(stats.mcp_response_bytes > 0)),
        );
    }

    println!(
        "\n  Stdout: {} (~{} tokens at 4 bytes/token), MCP sent after compaction: {}",
        format_bytes(report.stdout_bytes, 1),
        report.stdout_bytes / 4,
        format_bytes(report.mcp_response_bytes, 1),
    );
    println!("  Errors: {}   Empty results: {}", report.errors, report.no_results);
    if report.index_missing > 0 {
        println!(
            "  {}",
            format!(
                "{} calls ran without an index — run 'ast-index rebuild'.",
                report.index_missing
            )
            .yellow()
        );
    }
    println!(
        "  {}",
        "Stdout is measured only when it is not a terminal (agents, MCP, pipes).".dimmed()
    );

    if !report.projects.is_empty() {
        println!("\n{}", "Projects:".bold());
        for project in &report.projects {
            println!("  {:>6}  {}", project.calls, project.project);
        }
    }
}

fn format_bytes(bytes: u64, measured: u64) -> String {
    if measured == 0 {
        return "-".into();
    }
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{:.1} MB", value / KB / KB)
    }
}

/// UTC calendar date for a unix-ms timestamp (civil-from-days, no chrono dep).
fn format_date(ms: i64) -> String {
    let days = ms.div_euclid(DAY_MS);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_parses_days_and_all() {
        assert_eq!(Window::parse("7d").unwrap(), Window::Days(7));
        assert_eq!(Window::parse("30").unwrap(), Window::Days(30));
        assert_eq!(Window::parse("ALL").unwrap(), Window::All);
        assert!(Window::parse("0d").is_err());
        assert!(Window::parse("week").is_err());
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(percentile(&values, 50), 5);
        assert_eq!(percentile(&values, 95), 10);
        assert_eq!(percentile(&[42], 95), 42);
        assert_eq!(percentile(&[], 50), 0);
    }

    #[test]
    fn format_date_handles_epoch_and_leap_day() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_709_164_800_000), "2024-02-29");
    }
}
