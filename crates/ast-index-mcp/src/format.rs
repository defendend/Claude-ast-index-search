//! Compact-text formatter for ast-index JSON responses.
//!
//! Reasoning: agent context is the scarcest resource. ast-index's
//! `--format json` produces pretty-printed JSON (whitespace + quoting) that
//! eats 2-3× the tokens of a plain-text summary carrying the same
//! information. For MCP use we default to a TOON-inspired compact format
//! and keep JSON as an opt-in via the `format: "json"` tool parameter.
//!
//! Size comparison on a typical `search` response (3 content matches):
//!   pretty JSON:   ~280 bytes, ~90 tokens
//!   compact JSON:  ~180 bytes, ~60 tokens
//!   this format:   ~120 bytes, ~35 tokens
//!
//! Shape detection is best-effort: anything we don't recognise falls
//! through to compact JSON (`serde_json::to_string`) which still beats
//! pretty JSON by ~40%.

use serde_json::Value;
use std::collections::HashSet;
use std::fmt::Write;

/// Format an ast-index JSON response as compact text.
///
/// `tool` is the MCP tool name (`search`, `usages`, etc.) and drives
/// shape-aware formatting. If the response shape doesn't match what we
/// expect for that tool, we fall back to compact JSON so no information
/// is lost.
pub fn to_compact(tool: &str, raw_json: &str) -> String {
    let value: Value = match serde_json::from_str(raw_json) {
        Ok(v) => v,
        // Not JSON (e.g. `outline` prints plain text) — pass through.
        Err(_) => return raw_json.trim_end().to_string(),
    };

    let mut out = String::with_capacity(raw_json.len() / 2);
    let rendered = match tool {
        "explore" => render_explore(&value, &mut out),
        "search" => render_search(&value, &mut out),
        "refs" => render_refs(&value, &mut out),
        "usages" | "callers" => render_ref_list(&value, &mut out),
        "symbol" | "class" | "implementations" => render_symbol_list(&value, &mut out),
        "file" | "find_file" => render_file_list(&value, &mut out),
        "stats" => render_stats(&value, &mut out),
        "changed" => render_changed(&value, &mut out),
        "hotspots" => render_hotspots(&value, &mut out),
        graph if graph.starts_with("graph_") => render_graph(&value, &mut out),
        _ => false,
    };

    if !rendered {
        return serde_json::to_string(&value).unwrap_or_else(|_| raw_json.to_string());
    }

    let trimmed = out.trim_end().to_string();
    if trimmed.is_empty() {
        "(no results)".to_string()
    } else {
        trimmed
    }
}

fn render_search(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if let Some(rank) = obj.get("rank").and_then(Value::as_object) {
        return render_ranked_search(obj, rank, out);
    }
    if obj.get("fallback").and_then(Value::as_str) == Some("explore") {
        return render_explore_fallback(obj, out);
    }

    let mut any_section = false;

    if let Some(files) = obj.get("files").and_then(Value::as_array) {
        // A files section of any other shape would be printed as a bare
        // heading; compact JSON keeps what this renderer does not know.
        if !files.iter().all(Value::is_string) {
            return false;
        }
        if !files.is_empty() {
            any_section = true;
            writeln!(out, "Files:").ok();
            for f in files {
                if let Some(s) = f.as_str() {
                    writeln!(out, "  {s}").ok();
                }
            }
            write_named_pagination_notice(obj, "files", out);
        }
    }

    if let Some(symbols) = obj.get("symbols").and_then(Value::as_array) {
        if !symbols.is_empty() {
            any_section = true;
            writeln!(out, "\nSymbols:").ok();
            for s in symbols {
                write_symbol_line(s, "  ", out);
            }
            write_named_pagination_notice(obj, "symbols", out);
        }
    }

    any_section |= write_search_references(obj, "References (usage counts):", out);
    any_section |= write_search_content(obj, "Content:", out);

    any_section || obj.contains_key("files")
}

fn write_search_references(
    obj: &serde_json::Map<String, Value>,
    heading: &str,
    out: &mut String,
) -> bool {
    let Some(refs) = obj.get("references").and_then(Value::as_array) else {
        return false;
    };
    if refs.is_empty() {
        return false;
    }
    writeln!(out, "\n{heading}").ok();
    for r in refs {
        if let (Some(name), Some(count)) = (
            r.get("name").and_then(Value::as_str),
            r.get("usage_count").and_then(Value::as_i64),
        ) {
            writeln!(out, "  {name} ×{count}").ok();
        }
    }
    write_named_pagination_notice(obj, "references", out);
    true
}

fn write_search_content(
    obj: &serde_json::Map<String, Value>,
    heading: &str,
    out: &mut String,
) -> bool {
    let Some(content) = obj.get("content_matches").and_then(Value::as_array) else {
        return false;
    };
    if content.is_empty() {
        return false;
    }
    writeln!(out, "\n{heading}").ok();
    for m in content {
        if let (Some(path), Some(line), Some(snippet)) = (
            m.get("path").and_then(Value::as_str),
            m.get("line").and_then(Value::as_i64),
            m.get("content").and_then(Value::as_str),
        ) {
            writeln!(out, "  {path}:{line}  {}", truncate(snippet, 100)).ok();
        }
    }
    write_named_pagination_notice(obj, "content_matches", out);
    true
}

// ---------------------------------------------------------------------------
// explore, and search falling back to it
// ---------------------------------------------------------------------------

/// `explore`: the report under a line naming the query.
fn render_explore(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if !obj.get("symbols").is_some_and(Value::is_array) {
        return false;
    }
    let query = obj.get("query").and_then(Value::as_str).unwrap_or("");
    writeln!(out, "explore: {query}").ok();
    render_explore_report(obj, out)
}

/// A multi-word `search` without literal matches answers with the `explore`
/// report instead, marked `fallback: "explore"`.
fn render_explore_fallback(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let reason = obj
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("no literal matches");
    writeln!(out, "fallback: explore — {reason}").ok();
    render_explore_report(obj, out)
}

/// The sections of an `explore` report: the source of the best functions and
/// an outline of the best types and modules, the ranked symbols, graph
/// neighbours and tests found by path convention.
fn render_explore_report(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    if let Some(files) = obj.get("files").and_then(Value::as_array) {
        if !files.is_empty() {
            writeln!(out, "\nSource:").ok();
        }
        for file in files {
            let path = file.get("path").and_then(Value::as_str).unwrap_or("?");
            let line = file.get("line").and_then(Value::as_i64).unwrap_or(0);
            let symbol = file.get("symbol").and_then(Value::as_str).unwrap_or("?");
            writeln!(out, "  {path}:{line} {symbol}").ok();
            let source = file.get("source").and_then(Value::as_str).unwrap_or("");
            for code in source.lines() {
                writeln!(out, "    {code}").ok();
            }
            render_explore_outline(file, out);
        }
    }

    if let Some(symbols) = obj.get("symbols").and_then(Value::as_array) {
        if !symbols.is_empty() {
            writeln!(out, "\nSymbols (by relevance):").ok();
        }
        for symbol in symbols {
            let name = symbol.get("name").and_then(Value::as_str).unwrap_or("?");
            let kind = symbol.get("kind").and_then(Value::as_str).unwrap_or("?");
            let path = symbol.get("path").and_then(Value::as_str).unwrap_or("?");
            let line = symbol.get("line").and_then(Value::as_i64).unwrap_or(0);
            let vendor = if symbol.get("vendor").and_then(Value::as_bool) == Some(true) {
                " (third-party)"
            } else {
                ""
            };
            writeln!(out, "  {name} [{kind}] {path}:{line}{vendor}").ok();
        }
    }

    if let Some(neighbours) = obj.get("neighbours").and_then(Value::as_array) {
        if !neighbours.is_empty() {
            writeln!(out, "\nGraph neighbours:").ok();
        }
        for neighbour in neighbours {
            let link = neighbour.get("link").and_then(Value::as_str).unwrap_or("?");
            let name = neighbour.get("name").and_then(Value::as_str).unwrap_or("?");
            let kind = neighbour.get("kind").and_then(Value::as_str).unwrap_or("?");
            let path = neighbour.get("path").and_then(Value::as_str).unwrap_or("?");
            let line = neighbour.get("line").and_then(Value::as_i64).unwrap_or(0);
            writeln!(out, "  [{link}] {name} [{kind}] {path}:{line}").ok();
        }
    }

    if let Some(tests) = obj.get("tests").and_then(Value::as_array) {
        if !tests.is_empty() {
            writeln!(out, "\nTests:").ok();
        }
        for entry in tests {
            let source = entry.get("source").and_then(Value::as_str).unwrap_or("?");
            let found: Vec<&str> = entry
                .get("tests")
                .and_then(Value::as_array)
                .map(|tests| tests.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if found.is_empty() {
                writeln!(out, "  {source} ← no test file found by convention").ok();
            } else {
                writeln!(out, "  {source} ← {}", found.join(", ")).ok();
            }
        }
    }
    true
}

/// The outline `explore` gives for a type or module instead of its source:
/// one `:start-end name [kind]` row per definition.
fn render_explore_outline(file: &Value, out: &mut String) {
    let Some(rows) = file.get("outline").and_then(Value::as_array) else {
        return;
    };
    for row in rows {
        let name = row.get("name").and_then(Value::as_str).unwrap_or("?");
        let kind = row.get("kind").and_then(Value::as_str).unwrap_or("?");
        let line = row.get("line").and_then(Value::as_i64).unwrap_or(0);
        let position = match row.get("end_line").and_then(Value::as_i64) {
            Some(end) if end > line => format!(":{line}-{end}"),
            _ => format!(":{line}"),
        };
        writeln!(out, "    {position} {name} [{kind}]").ok();
    }
    let hidden = file
        .get("outline_hidden")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if hidden > 0 {
        writeln!(out, "    … {hidden} more").ok();
    }
}

// ---------------------------------------------------------------------------
// search --rank
// ---------------------------------------------------------------------------

/// Evidence already printed in this response. History is per file and a
/// file result borrows the graph numbers of its strongest symbol, so the same
/// history or graph line would otherwise repeat under a file and again under
/// each of its symbols.
#[derive(Default)]
struct PrintedEvidence {
    histories: HashSet<String>,
    graphs: HashSet<String>,
}

/// `search --rank` wraps the plain report: files become `{path, rank}`
/// objects and symbols gain a `rank` dossier, `null` when the preset was not
/// applied.
fn render_ranked_search(
    obj: &serde_json::Map<String, Value>,
    rank: &serde_json::Map<String, Value>,
    out: &mut String,
) -> bool {
    let preset = rank.get("preset").and_then(Value::as_str).unwrap_or("?");
    if rank.get("applied").and_then(Value::as_bool) == Some(true) {
        write_rank_header(preset, rank, out);
    } else {
        write_rank_not_applied(preset, rank, out);
    }

    let mut printed = PrintedEvidence::default();
    if let Some(files) = obj.get("files").and_then(Value::as_array) {
        if !files.is_empty() {
            writeln!(out, "\nFiles:").ok();
            for file in files {
                let path = file
                    .as_str()
                    .or_else(|| file.get("path").and_then(Value::as_str))
                    .unwrap_or("?");
                writeln!(out, "  {path}").ok();
                if let Some(dossier) = file.get("rank").filter(|d| d.is_object()) {
                    write_dossier(dossier, preset, path, None, &mut printed, out);
                }
            }
            write_named_pagination_notice(obj, "files", out);
        }
    }
    if let Some(symbols) = obj.get("symbols").and_then(Value::as_array) {
        if !symbols.is_empty() {
            writeln!(out, "\nSymbols:").ok();
            for symbol in symbols {
                write_symbol_line(symbol, "  ", out);
                if let Some(dossier) = symbol.get("rank").filter(|d| d.is_object()) {
                    let path = symbol.get("path").and_then(Value::as_str).unwrap_or("?");
                    let key = evidence_key(
                        symbol.get("name").and_then(Value::as_str).unwrap_or("?"),
                        path,
                        symbol.get("line").and_then(Value::as_i64).unwrap_or(0),
                    );
                    write_dossier(dossier, preset, path, Some(key), &mut printed, out);
                }
            }
            write_named_pagination_notice(obj, "symbols", out);
        }
    }
    write_search_references(obj, "References (usage counts, not ranked):", out);
    write_search_content(obj, "Content (not ranked):", out);
    true
}

fn write_rank_header(preset: &str, rank: &serde_json::Map<String, Value>, out: &mut String) {
    let formula = rank.get("formula").and_then(Value::as_str).unwrap_or("");
    writeln!(out, "Ranked by {preset} = {formula}.").ok();

    let mut evidence = Vec::new();
    if let Some(history) = rank.get("history").filter(|h| h.is_object()) {
        let commits = history
            .get("commits_analyzed")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let head = history
            .get("head")
            .and_then(Value::as_str)
            .map(short_sha)
            .unwrap_or("?");
        evidence.push(format!(
            "history is per file ({commits} commits, HEAD {head})"
        ));
    }
    if rank.get("graph").is_some_and(Value::is_object) {
        evidence.push("graph numbers are per symbol".to_string());
    }
    evidence.push("pNN = percentile within this repo".to_string());
    if let Some(pool) = rank.get("pool") {
        evidence.push(format!(
            "re-ranked the top {} symbols and {} files by relevance, exact names first",
            pool.get("symbols").and_then(Value::as_u64).unwrap_or(0),
            pool.get("files").and_then(Value::as_u64).unwrap_or(0),
        ));
        if pool.get("tests_excluded").and_then(Value::as_bool) == Some(true) {
            evidence.push("test files left out".to_string());
        }
    }
    writeln!(out, "{}.", capitalize(&evidence.join("; "))).ok();

    if rank
        .get("graph")
        .and_then(|g| g.get("stale"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        writeln!(
            out,
            "warning: the symbol graph is stale (the index changed since it was built); call graph_build for current numbers."
        )
        .ok();
    }
    for warning in string_items(rank.get("warnings")) {
        if !warning.contains("stale") {
            writeln!(out, "warning: {warning}").ok();
        }
    }
}

fn write_rank_not_applied(preset: &str, rank: &serde_json::Map<String, Value>, out: &mut String) {
    writeln!(
        out,
        "Ranking '{preset}' NOT applied: results are in plain relevance order. Missing:"
    )
    .ok();
    for missing in rank
        .get("missing")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let reason = missing
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("evidence");
        let command = missing.get("command").and_then(Value::as_str).unwrap_or("");
        let remedy = match missing.get("signal").and_then(Value::as_str) {
            Some("graph") => "call graph_build (takes seconds)".to_string(),
            Some("history") => format!(
                "run `{command}` in a shell (not available through MCP; the first run reads the whole history)"
            ),
            _ => format!("run `{command}`"),
        };
        writeln!(out, "  - {reason}: {remedy}").ok();
    }
}

fn write_dossier(
    dossier: &Value,
    preset: &str,
    path: &str,
    symbol_key: Option<String>,
    printed: &mut PrintedEvidence,
    out: &mut String,
) {
    let tier = dossier.get("tier").and_then(Value::as_str).unwrap_or("?");
    let position = match dossier.get("relevance_rank").and_then(Value::as_u64) {
        Some(position) => format!("relevance #{position}, {tier}"),
        None => format!("match: {tier}"),
    };
    let head = match (
        dossier.get("score").and_then(Value::as_f64),
        dossier.get("unscored").and_then(Value::as_str),
    ) {
        (Some(score), _) => format!(
            "{preset} {score:.2} = {} · {position}",
            combine_components(preset, dossier.get("components"))
        ),
        (None, Some(reason)) => format!(
            "{preset} not scored: {} · {position}",
            describe_unscored(reason)
        ),
        (None, None) => position,
    };
    writeln!(out, "    {head}").ok();

    let mut repeated: Vec<String> = Vec::new();
    if let Some(history) = dossier.get("history").filter(|h| h.is_object()) {
        if printed.histories.insert(path.to_string()) {
            writeln!(out, "    history: {}", history_line(history)).ok();
        } else {
            repeated.push("history".to_string());
        }
    }

    if let Some(graph) = dossier.get("graph").filter(|g| g.is_object()) {
        let strongest = graph.get("strongest_symbol").filter(|s| s.is_object());
        let (label, key) = match strongest {
            Some(symbol) => {
                let name = symbol.get("name").and_then(Value::as_str).unwrap_or("?");
                let kind = symbol.get("kind").and_then(Value::as_str).unwrap_or("?");
                let line = symbol.get("line").and_then(Value::as_i64).unwrap_or(0);
                (
                    format!("graph via {name} [{kind}]:{line}"),
                    evidence_key(name, path, line),
                )
            }
            None => (
                "graph".to_string(),
                symbol_key.unwrap_or_else(|| format!("file {path}")),
            ),
        };
        if printed.graphs.insert(key) {
            writeln!(out, "    {label}: {}", graph_dossier_line(graph)).ok();
        } else {
            repeated.push(label);
        }
    }
    if !repeated.is_empty() {
        writeln!(out, "    {}: as above", repeated.join(" and ")).ok();
    }
}

fn evidence_key(name: &str, path: &str, line: i64) -> String {
    format!("{name}@{path}:{line}")
}

fn combine_components(preset: &str, components: Option<&Value>) -> String {
    let parts: Vec<String> = components
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|component| {
            Some(format!(
                "{} {:.2}",
                component.get("name")?.as_str()?,
                component.get("value")?.as_f64()?
            ))
        })
        .collect();
    match preset {
        "proven" => format!("mean({})", parts.join(", ")),
        "risky" => parts.join(" × "),
        _ => parts.join(", "),
    }
}

fn describe_unscored(reason: &str) -> &str {
    match reason {
        "vendor" => "third-party code",
        "extra_root" => "outside the primary root, which is all the history covers",
        "no_history" => {
            "no collected history for this file (untracked or newer than the last collection)"
        }
        other => other,
    }
}

fn history_line(history: &Value) -> String {
    let int = |key: &str| history.get(key).and_then(Value::as_i64).unwrap_or(0);
    let commits = int("commits");
    let mut parts = vec![
        format!("{commits} commits p{}", int("commits_pct")),
        format!(
            "fixes {}/{commits}={:.0}% p{}",
            int("fix_commits"),
            history
                .get("fix_ratio")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                * 100.0,
            int("fix_ratio_pct")
        ),
        format!("churn {} p{}", int("churn"), int("churn_pct")),
        format!("{} authors p{}", int("authors"), int("authors_pct")),
    ];
    if let Some(age) = history.get("age_days").and_then(Value::as_f64) {
        parts.push(format!("age {age:.0}d p{}", int("age_pct")));
    }
    if let Some(idle) = history.get("days_since_change").and_then(Value::as_f64) {
        parts.push(format!("changed {idle:.0}d ago p{}", int("idle_pct")));
    }
    push_labels(&mut parts, history.get("labels"));
    parts.join(" · ")
}

fn graph_dossier_line(graph: &Value) -> String {
    if graph.get("in_graph").and_then(Value::as_bool) == Some(false) {
        return "no resolved or ambiguous edges".to_string();
    }
    let int = |key: &str| graph.get(key).and_then(Value::as_u64).unwrap_or(0);
    let pct = |key: &str| graph.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let ambiguous = int("fan_in_ambiguous");
    let mut parts = if int("fan_in") == 0 {
        // Percentiles are measured among referenced symbols, so every one of
        // them is 0 here and would only repeat that nothing resolves to it.
        let mut none = "no resolved callers".to_string();
        if ambiguous > 0 {
            write!(none, " (+{ambiguous} ambiguous)").ok();
        }
        vec![none]
    } else {
        let mut fan_in = format!(
            "fan-in {} from {} files p{:.0}",
            int("fan_in"),
            int("fan_in_files"),
            pct("fan_in_files_pct")
        );
        if ambiguous > 0 {
            write!(fan_in, " (+{ambiguous} ambiguous)").ok();
        }
        vec![
            fan_in,
            format!(
                "dependents {} p{:.0}",
                int("dependents"),
                pct("dependents_pct")
            ),
            format!("pagerank p{:.0}", pct("pagerank_pct")),
        ]
    };
    push_labels(&mut parts, graph.get("labels"));
    parts.join(" · ")
}

fn push_labels(parts: &mut Vec<String>, labels: Option<&Value>) {
    let labels: Vec<&str> = string_items(labels).collect();
    if !labels.is_empty() {
        parts.push(labels.join(" "));
    }
}

// ---------------------------------------------------------------------------
// hotspots
// ---------------------------------------------------------------------------

const HISTORY_NOT_COLLECTED: &str = "No Git history collected for this index yet. \
Collection is not available through MCP: run `ast-index hotspots --collect` in the project \
(the first run reads the whole history, about a minute on a large monorepo; later runs are \
incremental), then call again.";

fn render_hotspots(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    let (Some(items), true) = (
        obj.get("items").and_then(Value::as_array),
        obj.contains_key("commits_analyzed"),
    ) else {
        return false;
    };
    let Some(head) = obj.get("head").and_then(Value::as_str) else {
        writeln!(out, "{HISTORY_NOT_COLLECTED}").ok();
        return true;
    };
    let number = |key: &str| obj.get(key).and_then(Value::as_u64).unwrap_or(0);
    writeln!(
        out,
        "Git hotspots: {} live files with history, {} commits (HEAD {}), sorted by {}{}; pNN = percentile within this repo",
        number("files_with_history"),
        number("commits_analyzed"),
        short_sha(head),
        obj.get("sort").and_then(Value::as_str).unwrap_or("score"),
        if obj.get("tests_excluded").and_then(Value::as_bool) == Some(true) {
            ", test files left out"
        } else {
            ""
        },
    )
    .ok();

    for hotspot in items {
        let int = |key: &str| hotspot.get(key).and_then(Value::as_i64).unwrap_or(0);
        let path = hotspot.get("path").and_then(Value::as_str).unwrap_or("?");
        let commits = int("commits");
        let mut line = format!(
            "score {} · {commits} commits p{} · fixes {}/{commits}={:.0}% p{} · churn +{}/-{} p{}",
            int("score"),
            int("commits_pct"),
            int("fix_commits"),
            hotspot
                .get("fix_ratio")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                * 100.0,
            int("fix_ratio_pct"),
            int("lines_added"),
            int("lines_deleted"),
            int("churn_pct"),
        );
        if let Some(relative) = hotspot.get("relative_churn").and_then(Value::as_f64) {
            write!(line, " ({relative:.1}x file)").ok();
        }
        write!(
            line,
            " · {} authors p{}",
            int("authors"),
            int("authors_pct")
        )
        .ok();
        if let Some(age) = hotspot.get("age_days").and_then(Value::as_f64) {
            write!(line, " · age {age:.0}d p{}", int("age_pct")).ok();
        }
        if let Some(idle) = hotspot.get("days_since_change").and_then(Value::as_f64) {
            write!(line, " · changed {idle:.0}d ago").ok();
        }
        if let Some(lines) = hotspot.get("current_lines").and_then(Value::as_i64) {
            write!(line, " · {lines} lines").ok();
        }
        writeln!(out, "{path}\n  {line}").ok();
        let labels: Vec<&str> = string_items(hotspot.get("labels")).collect();
        if !labels.is_empty() {
            writeln!(out, "  {}", labels.join(" ")).ok();
        }
    }
    if items.is_empty() {
        writeln!(out, "No files matched the filters.").ok();
    }
    write_more_notice(obj.get("pagination"), out);
    true
}

// ---------------------------------------------------------------------------
// graph
// ---------------------------------------------------------------------------

/// Matched definitions listed in full before the list is cut, for queries
/// whose name matched several definitions.
const MATCHED_SHOWN: usize = 8;

/// Every `graph_*` tool; the report shape tells the subcommand apart
/// (`graph_dependents` returns either direct edges or an impact report).
fn render_graph(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if let Some(error) = obj.get("error").and_then(Value::as_str) {
        writeln!(out, "{}", graph_error_hint(error)).ok();
        return true;
    }
    if obj.contains_key("nodes") && obj.contains_key("by_confidence") {
        return render_graph_summary(obj, out);
    }
    if obj
        .get("graph")
        .and_then(|g| g.get("stale"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        writeln!(
            out,
            "warning: the symbol graph is stale (the index changed since it was built); results may be outdated — repeat with refresh: true."
        )
        .ok();
    }
    if obj.contains_key("levels") {
        render_graph_impact(obj, out)
    } else if obj.contains_key("from") && obj.contains_key("to") {
        render_graph_path(obj, out)
    } else if obj.contains_key("direction") && obj.contains_key("matched") {
        render_graph_edges(obj, out)
    } else if obj.contains_key("components") {
        render_graph_cycles(obj, out)
    } else if obj.contains_key("graph") && obj.contains_key("items") {
        // `top` carries its sort key; `metrics` is the bare graph page.
        render_graph_metrics(obj, out)
    } else {
        false
    }
}

fn graph_error_hint(error: &str) -> String {
    if error.starts_with("symbol graph not built") {
        "Symbol graph not built for this index. Repeat the call with refresh: true, or call graph_build (takes seconds).".to_string()
    } else if error.starts_with("no symbol matches") {
        format!(
            "{}. Symbol specs are `Name`, `Outer::Name` or `Class#member`; `symbol` or `search` find the exact name.",
            capitalize(error)
        )
    } else {
        capitalize(error)
    }
}

fn render_graph_summary(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let number = |key: &str| obj.get(key).and_then(Value::as_u64).unwrap_or(0);
    writeln!(
        out,
        "Symbol graph built: {} nodes, {} edges ({} resolved) in {} ms.",
        number("nodes"),
        number("edges"),
        number("resolved_edges"),
        number("elapsed_ms")
    )
    .ok();
    let levels: Vec<String> = obj
        .get("by_confidence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|level| {
            let edges = level.get("edges")?.as_u64()?;
            let confidence = level.get("confidence")?.as_str()?;
            (edges > 0).then(|| format!("{confidence} {edges}"))
        })
        .collect();
    if !levels.is_empty() {
        writeln!(out, "Edges by confidence: {}.", levels.join(" · ")).ok();
    }
    let seen = number("references_seen");
    let linked = number("references_linked");
    if seen > 0 {
        writeln!(
            out,
            "References: {seen} seen, {linked} linked ({:.1}%).",
            100.0 * linked as f64 / seen as f64
        )
        .ok();
    }
    true
}

fn render_graph_edges(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let (Some(matched), Some(items)) = (
        obj.get("matched").and_then(Value::as_array),
        obj.get("items").and_then(Value::as_array),
    ) else {
        return false;
    };
    let dependents = obj.get("direction").and_then(Value::as_str) == Some("dependents");
    let members = obj.get("members").and_then(Value::as_bool) == Some(true);
    let include_ambiguous = obj.get("include_ambiguous").and_then(Value::as_bool) == Some(true);
    let resolved = obj
        .get("resolved_edges")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let ambiguous = obj
        .get("ambiguous_edges")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    write_matched(
        if dependents {
            "Dependents of"
        } else {
            "Dependencies of"
        },
        matched,
        out,
    );
    writeln!(
        out,
        "{resolved} resolved, {ambiguous} ambiguous edges{}{}",
        if members {
            " (class members included)"
        } else {
            ""
        },
        if ambiguous > 0 && !include_ambiguous {
            "; include_ambiguous: true lists the ambiguous ones"
        } else {
            ""
        }
    )
    .ok();
    write_graph_notes(obj, out);

    let per_subject = matched.len() > 1 || members;
    for item in items {
        let other = &item["other"];
        let reference_line = item.get("line").and_then(Value::as_i64).unwrap_or(0);
        let shown_line = if dependents {
            reference_line
        } else {
            other.get("line").and_then(Value::as_i64).unwrap_or(0)
        };
        let mut line = format!(
            "{} {} [{}] {}:{shown_line}",
            confidence_label(item),
            other.get("name").and_then(Value::as_str).unwrap_or("?"),
            other.get("kind").and_then(Value::as_str).unwrap_or("?"),
            other.get("path").and_then(Value::as_str).unwrap_or("?"),
        );
        let references = item.get("references").and_then(Value::as_u64).unwrap_or(1);
        if references > 1 {
            write!(line, " ×{references}").ok();
        }
        if per_subject {
            let subject = item["subject"]
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("?");
            if dependents {
                write!(line, " (on {subject})").ok();
            } else {
                write!(line, " (from {subject}, line {reference_line})").ok();
            }
        }
        writeln!(out, "{line}").ok();
    }
    if items.is_empty() {
        writeln!(out, "(no edges)").ok();
    }
    write_more_notice(obj.get("pagination"), out);
    true
}

fn confidence_label(item: &Value) -> String {
    match item.get("confidence").and_then(Value::as_str) {
        Some("ambiguous") => format!(
            "[ambiguous 1/{}]",
            item.get("candidates").and_then(Value::as_u64).unwrap_or(0)
        ),
        Some(level) => format!("[{level}]"),
        None => "[?]".to_string(),
    }
}

fn render_graph_impact(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let (Some(matched), Some(levels), Some(items)) = (
        obj.get("matched").and_then(Value::as_array),
        obj.get("levels").and_then(Value::as_array),
        obj.get("items").and_then(Value::as_array),
    ) else {
        return false;
    };
    let depth = obj.get("depth").and_then(Value::as_u64).unwrap_or(0);
    let include_ambiguous = obj.get("include_ambiguous").and_then(Value::as_bool) == Some(true);
    let members = obj.get("members").and_then(Value::as_bool) == Some(true);
    write_matched("Impact of", matched, out);

    let mut summary: Vec<String> = levels
        .iter()
        .map(|level| {
            let number = |key: &str| level.get(key).and_then(Value::as_u64).unwrap_or(0);
            format!(
                "depth {}: {} symbols in {} files",
                number("depth"),
                number("symbols"),
                number("files")
            )
        })
        .collect();
    summary.push(format!(
        "total {} symbols in {} files",
        obj.get("total_symbols")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        obj.get("total_files").and_then(Value::as_u64).unwrap_or(0)
    ));
    writeln!(
        out,
        "Transitive dependents up to depth {depth} over {} edges: {}",
        if include_ambiguous {
            "resolved + ambiguous"
        } else {
            "resolved"
        },
        summary.join(" · ")
    )
    .ok();
    if let (Some(symbols), Some(files)) = (
        obj.get("resolved_only_symbols").and_then(Value::as_u64),
        obj.get("resolved_only_files").and_then(Value::as_u64),
    ) {
        writeln!(
            out,
            "resolved edges alone: {symbols} symbols in {files} files; the rest is an upper bound through ambiguous names"
        )
        .ok();
    }
    write_graph_notes(obj, out);

    let via_is_the_seed = matched.len() == 1 && !members;
    for item in items {
        let item_depth = item.get("depth").and_then(Value::as_u64).unwrap_or(0);
        let mut line = format!("d{item_depth} {}", symbol_ref(&item["symbol"]));
        if !(via_is_the_seed && item_depth == 1) {
            if let Some(via) = item.get("via").and_then(Value::as_str) {
                write!(line, " via {via}").ok();
            }
        }
        if let Some(confidence) = item.get("confidence").and_then(Value::as_str) {
            write!(line, " ({confidence})").ok();
        }
        writeln!(out, "{line}").ok();
    }
    if items.is_empty() {
        writeln!(out, "(nothing depends on it within depth {depth})").ok();
    }
    write_more_notice(obj.get("pagination"), out);
    true
}

/// The `notes` a graph report carries: what its answer leaves out.
fn write_graph_notes(obj: &serde_json::Map<String, Value>, out: &mut String) {
    for note in string_items(obj.get("notes")) {
        writeln!(out, "note: {note}").ok();
    }
}

fn render_graph_path(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let (Some(from), Some(to), Some(paths)) = (
        obj.get("from").and_then(Value::as_array),
        obj.get("to").and_then(Value::as_array),
        obj.get("items").and_then(Value::as_array),
    ) else {
        return false;
    };
    let from = matched_names(from);
    let to = matched_names(to);
    let length = obj.get("length").and_then(Value::as_u64);
    let direction = obj.get("direction").and_then(Value::as_str);
    let shortest = obj
        .get("shortest_paths")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    match (length, direction) {
        (Some(length), Some(direction)) => {
            let counts = format!(
                "{length} hop(s), {shortest} shortest path(s), {} shown",
                paths.len()
            );
            if direction == "reverse" {
                writeln!(
                    out,
                    "No path {from} -> {to}; the reverse direction connects them: {to} -> {from} in {counts}"
                )
                .ok();
            } else {
                writeln!(out, "{from} -> {to}: {counts}").ok();
            }
        }
        _ => {
            let hint = if obj.get("include_ambiguous").and_then(Value::as_bool) == Some(true) {
                ""
            } else {
                " over resolved edges (include_ambiguous: true also follows ambiguous names)"
            };
            writeln!(
                out,
                "No dependency path between {from} and {to} within the hop limit{hint}; raise max_depth to look further."
            )
            .ok();
            return true;
        }
    }
    for (index, path) in paths.iter().enumerate() {
        writeln!(out, "path {}:", index + 1).ok();
        let mut previous_edge: Option<&str> = None;
        for hop in path.as_array().into_iter().flatten() {
            let symbol = symbol_ref(&hop["symbol"]);
            match previous_edge {
                None => writeln!(out, "  {symbol}").ok(),
                Some(edge) => writeln!(out, "  -[{edge}]-> {symbol}").ok(),
            };
            previous_edge = hop.get("edge").and_then(Value::as_str);
        }
    }
    true
}

fn render_graph_cycles(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let Some(items) = obj.get("items").and_then(Value::as_array) else {
        return false;
    };
    let components = obj.get("components").and_then(Value::as_u64).unwrap_or(0);
    if components == 0 {
        writeln!(out, "No dependency cycles over resolved edges.").ok();
        return true;
    }
    writeln!(
        out,
        "{components} dependency cycle(s) over resolved edges, largest first:"
    )
    .ok();
    for item in items {
        let size = item.get("size").and_then(Value::as_u64).unwrap_or(0);
        let chain: Vec<&str> = item
            .get("example")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|hop| hop.get("name").and_then(Value::as_str))
            .collect();
        writeln!(
            out,
            "{size} symbols in {} files: {}",
            item.get("files").and_then(Value::as_u64).unwrap_or(0),
            chain.join(" -> ")
        )
        .ok();
        let members = item
            .get("members")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for member in members {
            writeln!(out, "  {}", symbol_ref(member)).ok();
        }
        if size as usize > members.len() {
            writeln!(out, "  … +{} more", size as usize - members.len()).ok();
        }
    }
    write_more_notice(obj.get("pagination"), out);
    true
}

fn render_graph_metrics(obj: &serde_json::Map<String, Value>, out: &mut String) -> bool {
    let Some(items) = obj.get("items").and_then(Value::as_array) else {
        return false;
    };
    let sort = obj.get("sort").and_then(Value::as_str);
    let depth = items
        .first()
        .and_then(|item| item.get("dependents_depth"))
        .and_then(Value::as_u64)
        .unwrap_or(3);
    let scope = format!("resolved edges; dependents = transitive within {depth} hops");
    match sort {
        Some(sort) => writeln!(out, "Top symbols by {sort} ({scope}):").ok(),
        None => writeln!(out, "Graph metrics ({scope}):").ok(),
    };
    for (index, item) in items.iter().enumerate() {
        let symbol = symbol_ref(&item["symbol"]);
        let (marker, indent) = match sort {
            Some(_) => (format!("{}. ", index + 1), "   "),
            None => (String::new(), "  "),
        };
        writeln!(out, "{marker}{symbol}").ok();
        writeln!(out, "{indent}{}", metrics_line(item)).ok();
    }
    if items.is_empty() {
        writeln!(
            out,
            "{}",
            if sort.is_some() {
                "No symbols matched."
            } else {
                "No symbol matches."
            }
        )
        .ok();
    }
    write_more_notice(obj.get("pagination"), out);
    true
}

fn metrics_line(item: &Value) -> String {
    let int = |key: &str| item.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut fan_in = format!("fan-in {} ({} files", int("fan_in"), int("fan_in_files"));
    if int("fan_in_ambiguous") > 0 {
        write!(fan_in, ", +{} ambiguous", int("fan_in_ambiguous")).ok();
    }
    fan_in.push(')');
    let mut fan_out = format!("fan-out {}", int("fan_out"));
    if int("fan_out_ambiguous") > 0 {
        write!(fan_out, " (+{} ambiguous)", int("fan_out_ambiguous")).ok();
    }
    format!(
        "{fan_in} · {fan_out} · dependents {} · pagerank {:.2} p{:.0}",
        int("dependents"),
        item.get("pagerank").and_then(Value::as_f64).unwrap_or(0.0),
        item.get("pagerank_pct")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    )
}

fn write_matched(title: &str, matched: &[Value], out: &mut String) {
    if let [only] = matched {
        writeln!(out, "{title} {}", symbol_ref(only)).ok();
        return;
    }
    writeln!(
        out,
        "{title} {} definitions (narrow with in_file or kind):",
        matched.len()
    )
    .ok();
    for symbol in matched.iter().take(MATCHED_SHOWN) {
        writeln!(out, "  {}", symbol_ref(symbol)).ok();
    }
    if matched.len() > MATCHED_SHOWN {
        writeln!(out, "  … +{} more", matched.len() - MATCHED_SHOWN).ok();
    }
}

/// `Name` for one match, `Name (+N more definitions)` for several.
fn matched_names(matched: &[Value]) -> String {
    let first = matched
        .first()
        .and_then(|symbol| symbol.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    match matched.len() {
        0 | 1 => first.to_string(),
        n => format!("{first} (+{} more definitions)", n - 1),
    }
}

fn symbol_ref(symbol: &Value) -> String {
    format!(
        "{} [{}] {}:{}",
        symbol.get("name").and_then(Value::as_str).unwrap_or("?"),
        symbol.get("kind").and_then(Value::as_str).unwrap_or("?"),
        symbol.get("path").and_then(Value::as_str).unwrap_or("?"),
        symbol.get("line").and_then(Value::as_i64).unwrap_or(0)
    )
}

/// Truncation notice for the newer reports: their totals run into the
/// thousands, so it asks for a larger page rather than for all of it.
fn write_more_notice(pagination: Option<&Value>, out: &mut String) {
    let Some(pagination) = pagination else {
        return;
    };
    if pagination.get("truncated").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let returned = pagination
        .get("returned")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = pagination
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(returned);
    writeln!(out, "… {returned} of {total} shown; raise limit for more").ok();
}

fn string_items(value: Option<&Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn short_sha(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn render_refs(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };

    let mut any = false;

    if let Some(defs) = obj.get("definitions").and_then(Value::as_array) {
        if !defs.is_empty() {
            any = true;
            writeln!(out, "Definitions:").ok();
            for d in defs {
                write_symbol_line(d, "  ", out);
            }
            write_named_pagination_notice(obj, "definitions", out);
        }
    }

    if let Some(imports) = obj.get("imports").and_then(Value::as_array) {
        if !imports.is_empty() {
            any = true;
            writeln!(out, "\nImports:").ok();
            for i in imports {
                if let (Some(path), Some(line)) = (
                    i.get("path").and_then(Value::as_str),
                    i.get("line").and_then(Value::as_i64),
                ) {
                    let sig = i.get("signature").and_then(Value::as_str).unwrap_or("");
                    if sig.is_empty() {
                        writeln!(out, "  {path}:{line}").ok();
                    } else {
                        writeln!(out, "  {path}:{line}  {}", truncate(sig, 80)).ok();
                    }
                }
            }
            write_named_pagination_notice(obj, "imports", out);
        }
    }

    if let Some(usages) = obj.get("usages").and_then(Value::as_array) {
        if !usages.is_empty() {
            any = true;
            writeln!(out, "\nUsages:").ok();
            for u in usages {
                write_ref_line(u, "  ", out);
            }
            write_named_pagination_notice(obj, "usages", out);
        }
    }

    any || obj.contains_key("definitions")
}

fn render_ref_list(v: &Value, out: &mut String) -> bool {
    // Legacy responses are arrays; schema v2 wraps them in {items,pagination}.
    let Some(arr) = page_items(v) else {
        return false;
    };
    for r in arr {
        write_ref_line(r, "", out);
    }
    write_page_pagination_notice(v, out);
    true
}

fn render_symbol_list(v: &Value, out: &mut String) -> bool {
    let Some(arr) = page_items(v) else {
        return false;
    };
    for s in arr {
        write_symbol_line(s, "", out);
    }
    write_page_pagination_notice(v, out);
    true
}

fn page_items(value: &Value) -> Option<&Vec<Value>> {
    value
        .as_array()
        .or_else(|| value.as_object()?.get("items").and_then(Value::as_array))
}

fn write_page_pagination_notice(value: &Value, out: &mut String) {
    if let Some(pagination) = value.get("pagination") {
        write_pagination_notice(pagination, out);
    }
}

fn write_named_pagination_notice(
    object: &serde_json::Map<String, Value>,
    name: &str,
    out: &mut String,
) {
    if let Some(pagination) = object
        .get("pagination")
        .and_then(Value::as_object)
        .and_then(|pages| pages.get(name))
    {
        write_pagination_notice(pagination, out);
    }
}

fn write_pagination_notice(pagination: &Value, out: &mut String) {
    if pagination.get("truncated").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let returned = pagination
        .get("returned")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = pagination
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(returned);
    writeln!(
        out,
        "  … truncated: showing {returned} of {total}; raise limit to {total}"
    )
    .ok();
}

fn render_file_list(v: &Value, out: &mut String) -> bool {
    let Some(arr) = v.as_array() else {
        return false;
    };
    for f in arr {
        if let Some(s) = f.as_str() {
            writeln!(out, "{s}").ok();
        }
    }
    true
}

fn render_stats(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };

    let project = obj.get("project").and_then(Value::as_str).unwrap_or("?");
    let db_size = obj
        .get("db_size_bytes")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let db_path = obj.get("db_path").and_then(Value::as_str).unwrap_or("");

    writeln!(out, "project: {project}").ok();
    if let Some(stats) = obj.get("stats").and_then(Value::as_object) {
        // Known counters first (stable order, skip zeros to save tokens).
        let keys = [
            "file_count",
            "symbol_count",
            "refs_count",
            "module_count",
            "xml_usages_count",
            "resources_count",
            "storyboard_usages_count",
            "ios_assets_count",
        ];
        for k in keys {
            if let Some(n) = stats.get(k).and_then(Value::as_i64) {
                if n > 0 {
                    writeln!(out, "{k}: {n}").ok();
                }
            }
        }
    }
    if db_size > 0 {
        writeln!(out, "db_size_mb: {:.2}", db_size as f64 / 1024.0 / 1024.0).ok();
    }
    if !db_path.is_empty() {
        writeln!(out, "db_path: {db_path}").ok();
    }
    true
}

fn render_changed(v: &Value, out: &mut String) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if obj.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return false;
    }

    let (Some(vcs), Some(base), Some(head), Some(scope_value), Some(changes)) = (
        obj.get("vcs").and_then(Value::as_str),
        obj.get("base").and_then(Value::as_str),
        obj.get("head").and_then(Value::as_str),
        obj.get("scope"),
        obj.get("changes").and_then(Value::as_array),
    ) else {
        return false;
    };
    let scope = match scope_value {
        Value::Null => ".",
        Value::String(scope) => scope,
        _ => return false,
    };

    let mut lines = Vec::with_capacity(changes.len());
    for change in changes {
        let (Some(status), Some(path)) = (
            change.get("status").and_then(Value::as_str),
            change.get("path").and_then(Value::as_str),
        ) else {
            return false;
        };
        let line = match status {
            "A" | "M" | "D" => format!("{status} {}", escape_path(path)),
            "R" => {
                let Some(old_path) = change.get("old_path").and_then(Value::as_str) else {
                    return false;
                };
                format!("R {} -> {}", escape_path(old_path), escape_path(path))
            }
            _ => return false,
        };
        lines.push(line);
    }

    let vcs = escape_path(vcs);
    let base = escape_path(base);
    let head = escape_path(head);
    let scope = escape_path(scope);
    writeln!(
        out,
        "Changed files from merge-base({base}, {head}) to {head} ({vcs}, scope={scope})"
    )
    .ok();
    if lines.is_empty() {
        writeln!(out, "(no changes)").ok();
    } else {
        for line in lines {
            writeln!(out, "{line}").ok();
        }
    }
    true
}

fn escape_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\0' => escaped.push_str("\\0"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{1b}' => escaped.push_str("\\x1b"),
            value if value.is_control() => {
                write!(escaped, "\\u{{{:x}}}", value as u32).ok();
            }
            value => escaped.push(value),
        }
    }
    escaped
}

fn write_symbol_line(s: &Value, indent: &str, out: &mut String) {
    let name = s
        .get("qualified_name")
        .and_then(Value::as_str)
        .or_else(|| s.get("name").and_then(Value::as_str))
        .unwrap_or("?");
    let kind = s.get("kind").and_then(Value::as_str).unwrap_or("?");
    let path = s.get("path").and_then(Value::as_str).unwrap_or("?");
    let line = s.get("line").and_then(Value::as_i64).unwrap_or(0);
    writeln!(out, "{indent}{name} [{kind}] {path}:{line}").ok();

    if let Some(sig) = s.get("signature").and_then(Value::as_str) {
        if !sig.is_empty() {
            writeln!(out, "{indent}  {}", truncate(sig, 80)).ok();
        }
    }
}

fn write_ref_line(r: &Value, indent: &str, out: &mut String) {
    let path = r.get("path").and_then(Value::as_str).unwrap_or("?");
    let line = r.get("line").and_then(Value::as_i64).unwrap_or(0);
    writeln!(out, "{indent}{path}:{line}").ok();

    if let Some(ctx) = r
        .get("context")
        .and_then(Value::as_str)
        .or_else(|| r.get("content").and_then(Value::as_str))
    {
        if !ctx.is_empty() {
            writeln!(out, "{indent}  {}", truncate(ctx, 80)).ok();
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- search shaper ---

    #[test]
    fn search_renders_all_four_sections_compactly() {
        let json = r#"{
            "files": ["src/a.rs", "src/b.rs"],
            "symbols": [
                {"name":"Foo","kind":"class","path":"src/a.rs","line":10}
            ],
            "references": [
                {"name":"Foo","usage_count":3}
            ],
            "content_matches": [
                {"path":"src/c.rs","line":42,"content":"let x = Foo::new();"}
            ]
        }"#;
        let out = to_compact("search", json);
        assert!(out.contains("Files:"));
        assert!(out.contains("src/a.rs"));
        assert!(out.contains("Symbols:"));
        assert!(out.contains("Foo [class] src/a.rs:10"));
        assert!(out.contains("References (usage counts):"));
        assert!(out.contains("Foo ×3"));
        assert!(out.contains("Content:"));
        assert!(out.contains("src/c.rs:42"));
    }

    #[test]
    fn search_with_empty_arrays_returns_no_results() {
        let json = r#"{
            "files":[],"symbols":[],"references":[],"content_matches":[]
        }"#;
        let out = to_compact("search", json);
        assert_eq!(out, "(no results)");
    }

    #[test]
    fn search_skips_missing_sections() {
        let json = r#"{"files": ["only/a.rs"]}"#;
        let out = to_compact("search", json);
        assert!(out.contains("Files:"));
        assert!(out.contains("only/a.rs"));
        assert!(!out.contains("Symbols:"));
        assert!(!out.contains("Content:"));
    }

    // --- refs shaper ---

    #[test]
    fn refs_renders_definitions_imports_usages() {
        let json = r#"{
            "definitions": [
                {"name":"Foo","kind":"class","path":"a.rs","line":5}
            ],
            "imports": [
                {"path":"b.rs","line":1,"signature":"use crate::Foo;"}
            ],
            "usages": [
                {"path":"c.rs","line":42,"context":"Foo::bar()"}
            ]
        }"#;
        let out = to_compact("refs", json);
        assert!(out.contains("Definitions:"));
        assert!(out.contains("Foo [class] a.rs:5"));
        assert!(out.contains("Imports:"));
        assert!(out.contains("b.rs:1"));
        assert!(out.contains("use crate::Foo;"));
        assert!(out.contains("Usages:"));
        assert!(out.contains("c.rs:42"));
        assert!(out.contains("Foo::bar()"));
    }

    // --- usages / callers (ref list) ---

    #[test]
    fn usages_renders_array_of_refs() {
        let json = r#"[
            {"path":"src/a.rs","line":10,"context":"foo();"},
            {"path":"src/b.rs","line":20,"context":"foo()"}
        ]"#;
        let out = to_compact("usages", json);
        assert!(out.contains("src/a.rs:10"));
        assert!(out.contains("src/b.rs:20"));
        assert!(out.contains("foo();"));
    }

    #[test]
    fn usages_renders_page_v2_and_truncation_notice() {
        let json = r#"{
            "schema_version":2,
            "items":[{"path":"src/a.rs","line":10,"context":"foo();"}],
            "pagination":{"total":4,"returned":1,"truncated":true,"limit":1}
        }"#;
        let out = to_compact("usages", json);
        assert!(out.contains("src/a.rs:10"));
        assert!(out.contains("showing 1 of 4"));
        assert!(out.contains("raise limit to 4"));
    }

    #[test]
    fn callers_uses_same_shaper_as_usages() {
        let json = r#"[{"path":"a.rs","line":1,"context":"foo()"}]"#;
        assert_eq!(to_compact("usages", json), to_compact("callers", json));
    }

    #[test]
    fn callers_page_v2_renders_content_field() {
        let json = r#"{
            "schema_version":2,
            "items":[{"path":"a.rs","line":1,"content":"foo()"}],
            "pagination":{"total":2,"returned":1,"truncated":true,"limit":1}
        }"#;
        let out = to_compact("callers", json);
        assert!(out.contains("a.rs:1"));
        assert!(out.contains("foo()"));
        assert!(out.contains("showing 1 of 2"));
    }

    // --- symbol / class / implementations (symbol list) ---

    #[test]
    fn symbol_renders_symbol_lines_with_signatures() {
        let json = r#"[
            {"name":"Foo","kind":"class","path":"a.rs","line":5,
             "signature":"struct Foo<T>"},
            {"name":"bar","kind":"function","path":"b.rs","line":10}
        ]"#;
        let out = to_compact("symbol", json);
        assert!(out.contains("Foo [class] a.rs:5"));
        assert!(out.contains("struct Foo<T>"));
        assert!(out.contains("bar [function] b.rs:10"));
    }

    #[test]
    fn symbol_renders_page_v2_without_losing_pagination() {
        let json = r#"{
            "schema_version":2,
            "items":[{"name":"Foo","kind":"class","path":"a.rs","line":5}],
            "pagination":{"total":3,"returned":1,"truncated":true,"limit":1}
        }"#;
        let out = to_compact("symbol", json);
        assert!(out.contains("Foo [class] a.rs:5"));
        assert!(out.contains("showing 1 of 3"));
    }

    #[test]
    fn implementations_uses_same_shape_as_symbol() {
        let json = r#"[{"name":"X","kind":"class","path":"a.rs","line":1}]"#;
        let a = to_compact("symbol", json);
        let b = to_compact("implementations", json);
        let c = to_compact("class", json);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    // --- find_file ---

    #[test]
    fn find_file_renders_path_list() {
        let json = r#"["src/a.rs","src/b.rs","tests/c.rs"]"#;
        let out = to_compact("find_file", json);
        assert!(out.contains("src/a.rs"));
        assert!(out.contains("src/b.rs"));
        assert!(out.contains("tests/c.rs"));
    }

    // --- stats ---

    #[test]
    fn stats_renders_project_counts_and_db_size() {
        let json = r#"{
            "project": "Rust",
            "stats": {
                "file_count": 100, "symbol_count": 5000,
                "refs_count": 30000, "module_count": 0,
                "xml_usages_count": 0, "resources_count": 0,
                "storyboard_usages_count": 0, "ios_assets_count": 0
            },
            "db_size_bytes": 1048576,
            "db_path": "/tmp/index.db"
        }"#;
        let out = to_compact("stats", json);
        assert!(out.contains("project: Rust"));
        assert!(out.contains("file_count: 100"));
        assert!(out.contains("symbol_count: 5000"));
        assert!(out.contains("refs_count: 30000"));
        // Zero-counts should be skipped to save tokens
        assert!(!out.contains("module_count: 0"));
        assert!(!out.contains("xml_usages_count"));
        assert!(out.contains("db_size_mb: 1.00"));
        assert!(out.contains("db_path: /tmp/index.db"));
    }

    // --- changed ---

    #[test]
    fn changed_renders_schema_v1_with_all_statuses_and_rename_paths() {
        let json = r#"{
            "schema_version": 1,
            "vcs": "git",
            "base": "origin/main",
            "head": "HEAD",
            "scope": "crates/ast-index-mcp",
            "changes": [
                {"status": "A", "path": "src/new.rs"},
                {"status": "M", "path": "src/lib.rs"},
                {"status": "D", "path": "src/old.rs"},
                {"status": "R", "path": "src/current.rs", "old_path": "src/former.rs"}
            ]
        }"#;
        assert_eq!(
            to_compact("changed", json),
            concat!(
                "Changed files from merge-base(origin/main, HEAD) to HEAD ",
                "(git, scope=crates/ast-index-mcp)\n",
                "A src/new.rs\n",
                "M src/lib.rs\n",
                "D src/old.rs\n",
                "R src/former.rs -> src/current.rs"
            )
        );
    }

    #[test]
    fn changed_empty_branch_is_explicit() {
        let json = r#"{
            "schema_version": 1,
            "vcs": "arc",
            "base": "trunk",
            "head": "HEAD",
            "scope": null,
            "changes": []
        }"#;
        assert_eq!(
            to_compact("changed", json),
            concat!(
                "Changed files from merge-base(trunk, HEAD) to HEAD (arc, scope=.)\n",
                "(no changes)"
            )
        );
    }

    #[test]
    fn changed_escapes_path_controls_but_preserves_unicode() {
        let json = r#"{
            "schema_version": 1,
            "vcs": "git\rcli",
            "base": "main\nbranch",
            "head": "HEAD\tref",
            "scope": "nested\t雪",
            "changes": [
                {"status": "M", "path": "src/tab\t雪.rs"},
                {
                    "status": "R",
                    "path": "src/new\u001b雪.rs",
                    "old_path": "src/old\nname.rs"
                }
            ]
        }"#;
        assert_eq!(
            to_compact("changed", json),
            concat!(
                "Changed files from merge-base(main\\nbranch, HEAD\\tref) to ",
                "HEAD\\tref (git\\rcli, scope=nested\\t雪)\n",
                "M src/tab\\t雪.rs\n",
                "R src/old\\nname.rs -> src/new\\x1b雪.rs"
            )
        );
    }

    #[test]
    fn changed_unknown_schema_falls_back_without_losing_json() {
        let json = r#"{"schema_version":2,"changes":[]}"#;
        let rendered: Value = serde_json::from_str(&to_compact("changed", json)).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        assert_eq!(rendered, original);
    }

    // --- fixtures captured from the real `ast-index --format json` ---
    //
    // `tests/fixtures/` holds unedited CLI output: most of it from a small
    // Ruby repository with a scripted Git history (symbol graph + collected
    // history), `graph_dependents_ambiguous.json` from this repository's own
    // index, and the `*_not_*` files from the same Ruby index before
    // `graph build` / `hotspots --collect` ran. `search_explore_fallback.json`
    // is a multi-word `search` without literal matches over a small Ruby
    // billing repository: functions come with their source, the `Invoice`
    // class with an outline; `explore.json` is `explore` on the same query.

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../tests/fixtures/", $name, ".json"))
        };
    }

    fn lines_starting_with(out: &str, prefix: &str) -> usize {
        out.lines()
            .filter(|line| line.trim_start().starts_with(prefix))
            .count()
    }

    #[test]
    fn hotspots_report_keeps_every_signal_on_two_lines_per_file() {
        let json = fixture!("hotspots");
        let out = to_compact("hotspots", json);
        assert!(out.starts_with(
            "Git hotspots: 13 live files with history, 11 commits (HEAD bcc236d01a), sorted by score"
        ));
        assert!(out.contains(concat!(
            "app/services/billing/charge_service.rb\n",
            "  score 96 · 5 commits p96 · fixes 3/5=60% p96 · churn +12/-3 p96 (1.7x file)",
            " · 5 authors p96 · age 1298d p58 · changed 450d ago · 9 lines\n",
            "  churn:high fixes:high authors:many\n"
        )));
        assert!(out.ends_with("… 3 of 13 shown; raise limit for more"));
        assert!(out.len() * 2 < json.len(), "not compact: {out}");
    }

    #[test]
    fn hotspots_without_collected_history_names_the_cli_command() {
        let out = to_compact("hotspots", fixture!("hotspots_not_collected"));
        assert!(out.contains("`ast-index hotspots --collect`"));
        assert!(out.contains("not available through MCP"));
    }

    #[test]
    fn graph_build_summary_lists_only_confidences_with_edges() {
        let out = to_compact("graph_build", fixture!("graph_build"));
        assert_eq!(
            out,
            concat!(
                "Symbol graph built: 17 nodes, 17 edges (17 resolved) in 0 ms.\n",
                "Edges by confidence: scoped 17.\n",
                "References: 35 seen, 17 linked (48.6%)."
            )
        );
    }

    #[test]
    fn graph_dependents_list_one_line_per_edge_at_the_reference() {
        assert_eq!(
            to_compact("graph_dependents", fixture!("graph_dependents")),
            concat!(
                "Dependents of ApplicationService [class] app/services/application_service.rb:1\n",
                "3 resolved, 0 ambiguous edges\n",
                "[scoped] Billing::ChargeService [class] app/services/billing/charge_service.rb:2\n",
                "[scoped] Billing::DraftService [class] app/services/billing/draft_service.rb:2\n",
                "[scoped] Billing::RefundService [class] app/services/billing/refund_service.rb:2"
            )
        );
    }

    #[test]
    fn graph_dependents_show_ambiguous_candidates_and_truncation() {
        let out = to_compact("graph_dependents", fixture!("graph_dependents_ambiguous"));
        assert!(out.contains("7 resolved, 6 ambiguous edges\n"));
        assert!(out.contains("[ambiguous 1/1] cmd_composables [function] src/commands/grep.rs:821"));
        assert!(
            !out.contains("include_ambiguous: true lists"),
            "ambiguous edges are already listed: {out}"
        );
        assert!(out.ends_with("… 9 of 13 shown; raise limit for more"));
    }

    #[test]
    fn stale_graph_is_flagged_with_the_refresh_remedy() {
        let out = to_compact("graph_dependents", fixture!("graph_dependents_stale"));
        let first = out.lines().next().unwrap();
        assert!(first.starts_with("warning: the symbol graph is stale"));
        assert!(first.contains("refresh: true"));
    }

    #[test]
    fn graph_dependencies_with_members_name_the_referring_member() {
        let out = to_compact("graph_dependencies", fixture!("graph_dependencies_members"));
        assert!(out.contains("2 resolved, 0 ambiguous edges (class members included)"));
        assert!(out.contains(
            "[scoped] Billing::ChargeService [class] app/services/billing/charge_service.rb:2 (from create, line 3)"
        ));
    }

    #[test]
    fn graph_notes_say_what_the_answer_leaves_out() {
        let edges = r#"{
            "graph": {"built": true, "stale": false},
            "direction": "dependents",
            "matched": [{"name": "users.email", "kind": "column", "path": "db/schema.rb", "line": 9}],
            "resolved_edges": 0, "ambiguous_edges": 0, "include_ambiguous": false,
            "members": false, "exclude_tests": true, "excluded_test_edges": 2,
            "notes": ["Column edges come only from reads inside the model.", "2 edge(s) from test files left out (--exclude-tests)."],
            "items": [], "pagination": {"total": 0, "returned": 0, "truncated": false, "limit": 50}
        }"#;
        let out = to_compact("graph_dependents", edges);
        assert!(
            out.contains(concat!(
                "0 resolved, 0 ambiguous edges\n",
                "note: Column edges come only from reads inside the model.\n",
                "note: 2 edge(s) from test files left out (--exclude-tests).\n"
            )),
            "{out}"
        );
        let impact = r#"{
            "graph": {"built": true, "stale": false},
            "matched": [{"name": "Invoice", "kind": "class", "path": "app/models/invoice.rb", "line": 1}],
            "depth": 2, "include_ambiguous": false, "members": false,
            "exclude_tests": true, "excluded_test_symbols": 3,
            "notes": ["3 dependent(s) in test files left out and not followed (--exclude-tests)."],
            "levels": [], "total_symbols": 0, "total_files": 0,
            "items": [], "pagination": {"total": 0, "returned": 0, "truncated": false, "limit": 50}
        }"#;
        let out = to_compact("graph_dependents", impact);
        assert!(
            out.contains("note: 3 dependent(s) in test files left out and not followed"),
            "{out}"
        );
    }

    #[test]
    fn graph_impact_summarizes_depths_and_names_the_hop_it_came_through() {
        let out = to_compact("graph_dependents", fixture!("graph_impact"));
        assert!(out.contains(
            "depth 1: 2 symbols in 2 files · depth 2: 5 symbols in 4 files · total 7 symbols in 4 files"
        ));
        assert!(out.contains("\nd1 Invoice [class] app/models/invoice.rb:1 (scoped)\n"));
        assert!(
            out.contains("\nd2 total [function] app/models/invoice.rb:4 via LineItem (scoped)\n")
        );
        assert!(out.ends_with("… 4 of 7 shown; raise limit for more"));
    }

    #[test]
    fn graph_path_draws_each_hop_with_the_edge_that_reaches_it() {
        assert_eq!(
            to_compact("graph_path", fixture!("graph_path")),
            concat!(
                "OrdersController -> Invoice: 3 hop(s), 1 shortest path(s), 1 shown\n",
                "path 1:\n",
                "  create [function] app/controllers/orders_controller.rb:2\n",
                "  -[scoped]-> Billing::ChargeService [class] app/services/billing/charge_service.rb:2\n",
                "  -[contains]-> call [function] app/services/billing/charge_service.rb:3\n",
                "  -[scoped]-> Invoice [class] app/models/invoice.rb:1"
            )
        );
    }

    #[test]
    fn graph_path_labels_a_reverse_connection() {
        let out = to_compact("graph_path", fixture!("graph_path_reverse"));
        assert!(out.starts_with(
            "No path Invoice -> OrdersController; the reverse direction connects them: OrdersController -> Invoice"
        ));
    }

    #[test]
    fn graph_path_without_connection_suggests_what_to_widen() {
        let out = to_compact("graph_path", fixture!("graph_path_none"));
        assert!(out.starts_with(
            "No dependency path between Billing::Report (+1 more definitions) and Invoice"
        ));
        assert!(out.contains("include_ambiguous: true"));
        assert!(out.contains("max_depth"));
    }

    #[test]
    fn graph_cycles_print_the_chain_and_members() {
        assert_eq!(
            to_compact("graph_cycles", fixture!("graph_cycles")),
            concat!(
                "1 dependency cycle(s) over resolved edges, largest first:\n",
                "2 symbols in 2 files: Invoice -> LineItem -> Invoice\n",
                "  Invoice [class] app/models/invoice.rb:1\n",
                "  LineItem [class] app/models/line_item.rb:1"
            )
        );
    }

    #[test]
    fn graph_top_numbers_rows_and_metrics_do_not() {
        let top = to_compact("graph_metrics", fixture!("graph_top"));
        assert!(top.starts_with("Top symbols by pagerank"));
        assert!(top.contains(concat!(
            "1. ApplicationService [class] app/services/application_service.rb:1\n",
            "   fan-in 3 (3 files) · fan-out 0 · dependents 8 · pagerank 2.98 p97\n"
        )));

        let metrics = to_compact("graph_metrics", fixture!("graph_metrics"));
        assert!(metrics.starts_with("Graph metrics"));
        assert_eq!(lines_starting_with(&metrics, "fan-in"), 3);
        assert!(metrics.contains("Sales::Report [class] app/services/sales/report.rb:2"));
        assert!(!metrics.contains("1. "));
    }

    #[test]
    fn graph_errors_point_at_mcp_remedies() {
        let not_built = to_compact("graph_dependents", fixture!("graph_not_built"));
        assert!(not_built.contains("refresh: true"));
        assert!(not_built.contains("graph_build"));

        let no_match = to_compact("graph_dependents", fixture!("graph_no_match"));
        assert!(no_match.starts_with("No symbol matches 'NoSuchThing'."));
        assert!(no_match.contains("`Class#member`"));
    }

    #[test]
    fn graph_unrecognised_shape_falls_back_to_compact_json() {
        let json = r#"{"graph":{"built":true,"stale":false},"unexpected":1}"#;
        let out = to_compact("graph_dependents", json);
        let rendered: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(rendered, serde_json::from_str::<Value>(json).unwrap());
    }

    #[test]
    fn ranked_search_prints_each_history_and_graph_once() {
        let json = fixture!("search_rank_proven");
        let out = to_compact("search", json);
        assert!(out.starts_with("Ranked by proven = mean of calm"));
        assert_eq!(lines_starting_with(&out, "history: "), 3, "{out}");
        assert_eq!(lines_starting_with(&out, "graph via "), 3, "{out}");
        assert_eq!(
            lines_starting_with(&out, "history and graph: as above"),
            3,
            "{out}"
        );
        assert!(out.contains(concat!(
            "  app/services/billing/charge_service.rb\n",
            "    proven 0.49 = mean(calm 0.04, age 0.58, idle 0.35, used 1.00) · match: file_name\n",
            "    history: 5 commits p96 · fixes 3/5=60% p96 · churn 15 p96 · 5 authors p96",
            " · age 1298d p58 · changed 450d ago p35 · churn:high fixes:high authors:many\n"
        )));
        assert!(out.contains(concat!(
            "  Billing::ChargeService [class] app/services/billing/charge_service.rb:2\n",
            "    class ChargeService < ApplicationService\n",
            "    proven 0.49 = mean(calm 0.04, age 0.58, idle 0.35, used 1.00) · relevance #3, name\n",
            "    history and graph: as above\n"
        )));
        assert!(out.contains("\nContent (not ranked):\n"));
        assert!(
            out.len() * 4 < json.len(),
            "not compact: {} bytes",
            out.len()
        );
    }

    #[test]
    fn ranked_search_not_applied_keeps_plain_results_and_names_remedies() {
        let out = to_compact("search", fixture!("search_rank_not_applied"));
        assert!(
            out.starts_with("Ranking 'risky' NOT applied: results are in plain relevance order.")
        );
        assert!(out.contains("the symbol graph has not been built: call graph_build"));
        assert!(out.contains(
            "git history has not been collected: run `ast-index hotspots --collect` in a shell"
        ));
        assert!(out.contains("\nFiles:\n  app/services/application_service.rb\n"));
        assert!(out.contains(
            "\nSymbols:\n  ApplicationService [class] app/services/application_service.rb:1\n"
        ));
        assert!(!out.contains('{'), "fell back to JSON: {out}");
    }

    #[test]
    fn ranked_search_explains_unscored_results() {
        let out = to_compact("search", fixture!("search_rank_unscored"));
        assert!(out.contains(concat!(
            "proven not scored: no collected history for this file ",
            "(untracked or newer than the last collection) · match: file_name"
        )));
        assert!(out.contains("graph via Billing::DraftService [class]:2: no resolved callers"));
        assert!(out.contains("\n    graph: as above\n"));
    }

    #[test]
    fn ranked_search_with_stale_graph_warns_once() {
        let out = to_compact("search", fixture!("search_rank_stale"));
        assert_eq!(lines_starting_with(&out, "warning:"), 1, "{out}");
        assert!(out.contains("call graph_build"));
        assert!(!out.contains("history"), "central uses no history: {out}");
    }

    #[test]
    fn ranked_search_survives_null_and_foreign_dossiers() {
        let json = r#"{
            "rank": {"preset": "risky", "applied": false, "missing": [{"signal": "x"}]},
            "files": [{"path": "a.rb", "rank": null}, "b.rb", {"rank": 3}],
            "symbols": [{"name": "A", "kind": "class", "path": "a.rb", "line": 1, "rank": {"tier": 7}}]
        }"#;
        let out = to_compact("search", json);
        assert!(out.contains("  a.rb\n  b.rb\n  ?\n"));
        assert!(out.contains("A [class] a.rb:1"));
    }

    #[test]
    fn plain_search_render_is_unchanged() {
        assert_eq!(
            to_compact("search", fixture!("search_plain")),
            concat!(
                "Files:\n",
                "  app/services/application_service.rb\n",
                "  app/services/billing/charge_service.rb\n",
                "  app/services/billing/draft_service.rb\n",
                "  … truncated: showing 3 of 9; raise limit to 9\n",
                "\n",
                "Symbols:\n",
                "  ApplicationService [class] app/services/application_service.rb:1\n",
                "    class ApplicationService\n",
                "  Billing::DraftService [class] app/services/billing/draft_service.rb:2\n",
                "    class DraftService < ApplicationService\n",
                "  Billing::ChargeService [class] app/services/billing/charge_service.rb:2\n",
                "    class ChargeService < ApplicationService\n",
                "  … truncated: showing 3 of 5; raise limit to 5\n",
                "\n",
                "Content:\n",
                "  spec/services/charge_service_spec.rb:1  RSpec.describe Billing::ChargeService do\n",
                "  spec/services/charge_service_spec.rb:3  Billing::ChargeService.call\n",
                "  app/services/billing/draft_service.rb:2  class DraftService < ApplicationService\n",
                "  … truncated: showing 3 of 9; raise limit to 9"
            )
        );
    }

    #[test]
    fn search_falling_back_to_explore_keeps_source_symbols_and_tests() {
        let json = fixture!("search_explore_fallback");
        let out = to_compact("search", json);
        assert!(
            out.starts_with(
                "fallback: explore — no literal matches for a multi-word query; \
                 results are ranked by relevance\n\nSource:\n"
            ),
            "{out}"
        );
        assert!(out.contains(concat!(
            "  app/services/billing/charge_service.rb:12 gateway\n",
            "       12\t    def gateway\n",
            "       13\t      @gateway ||= PaymentGateway.new\n",
            "       14\t    end\n",
        )));
        assert!(out.contains(concat!(
            "  app/models/invoice.rb:1 Invoice\n",
            "    :1-13 Invoice [class]\n",
            "    :2-4 paid? [function]\n",
            "    :6-8 mark_paid! [function]\n",
            "    :10-12 mark_refunded! [function]\n",
        )));
        assert!(!out.contains("    … "), "{out}");
        let value: Value = serde_json::from_str(json).unwrap();
        for symbol in value["symbols"].as_array().unwrap() {
            let line = format!(
                "  {} [{}] {}:{}\n",
                symbol["name"].as_str().unwrap(),
                symbol["kind"].as_str().unwrap(),
                symbol["path"].as_str().unwrap(),
                symbol["line"]
            );
            assert!(out.contains(&line), "missing {line:?} in {out}");
        }
        assert!(out.contains(
            "  app/services/billing/charge_service.rb ← spec/services/billing/charge_service_spec.rb\n"
        ));
        assert!(out.contains("  app/models/invoice.rb ← no test file found by convention"));
        assert!(out.len() < json.len(), "not compact: {out}");
    }

    #[test]
    fn explore_renders_as_compact_text_not_json() {
        let json = fixture!("explore");
        let out = to_compact("explore", json);
        assert!(
            out.starts_with("explore: charge invoice gateway\n\nSource:\n"),
            "{out}"
        );
        assert!(out.contains(concat!(
            "  app/services/billing/charge_service.rb:12 gateway\n",
            "       12\t    def gateway\n",
        )));
        assert!(out.contains(concat!(
            "  app/models/invoice.rb:1 Invoice\n",
            "    :1-13 Invoice [class]\n",
            "    :2-4 paid? [function]\n",
        )));
        assert!(out.contains(
            "\nSymbols (by relevance):\n  charge [function] app/models/payment_gateway.rb:2\n"
        ));
        assert!(out.contains(
            "  app/services/billing/charge_service.rb ← spec/services/billing/charge_service_spec.rb\n"
        ));
        assert!(!out.contains("fallback"), "{out}");
        assert!(serde_json::from_str::<Value>(&out).is_err(), "{out}");
        assert!(out.len() < json.len() / 2, "not compact: {out}");
    }

    #[test]
    fn explore_of_an_unknown_shape_falls_back_to_compact_json() {
        let json = r#"{"query":"x","files":[]}"#;
        let rendered: Value = serde_json::from_str(&to_compact("explore", json)).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        assert_eq!(rendered, original);
    }

    #[test]
    fn explore_outline_counts_the_rows_it_left_out() {
        let json = r#"{"fallback":"explore","reason":"r","files":[{"path":"a.rb","line":1,
            "symbol":"A","outline":[{"name":"A","kind":"class","line":1,"end_line":90},
            {"name":"go","kind":"function","line":3,"end_line":3}],"outline_hidden":7}]}"#;
        let out = to_compact("search", json);
        assert!(
            out.contains("  a.rb:1 A\n    :1-90 A [class]\n    :3 go [function]\n    … 7 more"),
            "{out}"
        );
    }

    #[test]
    fn search_files_of_an_unknown_shape_fall_back_to_compact_json() {
        let json = r#"{"files":[{"path":"a.rb","line":1}],"symbols":[]}"#;
        let rendered: Value = serde_json::from_str(&to_compact("search", json)).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        assert_eq!(rendered, original);
    }

    // --- fall-through behaviour ---

    #[test]
    fn unknown_tool_falls_through_to_compact_json() {
        // Tools without a shaper fall back to serde_json::to_string (compact)
        let json = r#"{"foo": "bar"}"#;
        let out = to_compact("xyz_no_shaper", json);
        assert!(out.contains("foo"));
        assert!(out.contains("bar"));
        // Compact (not pretty-printed)
        assert!(!out.contains("\n  "));
    }

    #[test]
    fn non_json_input_passes_through_trim_end_only() {
        // outline returns plain text with possible indentation that
        // matters — leading whitespace is preserved, only trailing
        // newlines are stripped.
        let plain = "  Foo [class] file.rs:10\n\n";
        let out = to_compact("outline", plain);
        assert_eq!(out, "  Foo [class] file.rs:10");
    }

    // --- truncate helper ---

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_appends_ellipsis() {
        assert_eq!(truncate("0123456789abcdef", 10), "0123456789…");
    }

    #[test]
    fn truncate_preserves_unicode_chars() {
        assert_eq!(truncate("πρακτικά", 5), "πρακτ…");
    }
}
