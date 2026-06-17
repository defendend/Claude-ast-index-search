//! `explore` — Stage A prototype.
//!
//! One-shot context for a query: rank the most relevant symbols, show their
//! source (read fresh from disk — never stored in the DB), their neighbours
//! (cross-references), and any tests located by path convention.
//!
//! Design goals (see RFC): language-agnostic, vendor-aware, honest about
//! tests. Stage A reuses only data already in the index (FTS + fuzzy +
//! inheritance + refs). It deliberately does NOT build a call graph or run
//! RWR ranking — that is Stage B.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use rusqlite::Connection;
use serde_json::json;

use super::PathResolver;
use crate::db::{self, SearchResult};

/// Candidates pulled per query term from FTS before ranking.
const SEED_PER_TERM: usize = 40;
/// Max symbols listed in the ranked "Relevant symbols" section.
const MAX_SYMBOLS_LISTED: usize = 15;
/// Hard cap on lines per source snippet (god-file / minified protection).
const SNIPPET_CAP_LINES: usize = 60;

struct Cand {
    sym: SearchResult,
    score: f64,
    vendor: bool,
}

pub fn cmd_explore(root: &Path, query: &[String], max_files: usize, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db(root)?;
    let resolver = PathResolver::from_conn(root, &conn);

    let raw = query.join(" ");
    let terms = tokenize(&raw);
    if terms.is_empty() {
        println!("explore: query has no usable terms (need identifiers >= 3 chars)");
        return Ok(());
    }

    // 1. Seed: FTS per term, fuzzy fallback when a term is thin. Dedup by (path,line).
    let mut cands: Vec<Cand> = Vec::new();
    let mut seen: HashSet<(String, i64)> = HashSet::new();
    for term in &terms {
        let mut hits = db::search_symbols(&conn, term, SEED_PER_TERM)?;
        if hits.len() < 3 {
            hits.extend(db::search_symbols_fuzzy(&conn, term, SEED_PER_TERM)?);
        }
        for s in hits {
            if !resolver.matches_filter(s.root_path.as_deref()) {
                continue;
            }
            if !seen.insert((s.path.clone(), s.line)) {
                continue;
            }
            let vendor = is_vendor(&s.path);
            cands.push(Cand {
                sym: s,
                score: 0.0,
                vendor,
            });
        }
    }
    if cands.is_empty() {
        println!("explore: no symbols matched '{}'", raw);
        return Ok(());
    }

    // 2. Dominant language from non-vendor candidates — used to down-rank
    //    cross-stack noise (the bug that makes codegraph drag JS into Ruby queries).
    let dom_lang = dominant_lang(&cands);

    // 3. Score and sort.
    for c in &mut cands {
        c.score = score(c, &terms, dom_lang.as_deref());
    }
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));

    // 4. Pick distinct source files from the top non-vendor candidates.
    let mut file_order: Vec<usize> = Vec::new();
    let mut chosen_paths: HashSet<String> = HashSet::new();
    for (i, c) in cands.iter().enumerate() {
        if c.vendor {
            continue;
        }
        if chosen_paths.contains(&c.sym.path) {
            continue;
        }
        chosen_paths.insert(c.sym.path.clone());
        file_order.push(i);
        if file_order.len() >= max_files {
            break;
        }
    }

    // 5. Tests by path convention for the single top non-vendor symbol's file(s).
    let mut tests: Vec<(String, Vec<String>)> = Vec::new();
    for &i in file_order.iter().take(max_files) {
        let rel = &cands[i].sym.path;
        let found = find_tests_by_convention(&conn, rel)?;
        tests.push((rel.clone(), found));
    }

    if format == "json" {
        return emit_json(root, &raw, dom_lang.as_deref(), &cands, &file_order, &tests);
    }

    emit_text(root, &raw, dom_lang.as_deref(), &cands, &file_order, &tests, &resolver);
    Ok(())
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

fn score(c: &Cand, terms: &[String], dom_lang: Option<&str>) -> f64 {
    let name_lc = c.sym.name.to_lowercase();
    let qual_lc = c
        .sym
        .qualified_name
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let stem = path_stem(&c.sym.path).to_lowercase();
    let path_lc = c.sym.path.to_lowercase();

    // Name/path signal, plus a count of how many DISTINCT query terms this
    // symbol matches anywhere. Multi-term corroboration is the core relevance
    // signal in Stage A — it stands in for the structural connectivity that
    // RWR provides in Stage B, and breaks the "common word" tokenization trap
    // (a getter named like one frequent term must not outrank the symbol that
    // matches the whole query).
    let mut signal = 0.0;
    let mut term_hits = 0u32;
    for t in terms {
        let mut hit = false;
        if name_lc == *t {
            signal += 50.0;
            hit = true;
        } else if name_lc.contains(t) {
            signal += 25.0;
            hit = true;
        }
        if qual_lc.contains(t) {
            signal += 12.0;
            hit = true;
        }
        if stem.contains(t) || path_lc.contains(t) {
            signal += 8.0;
            hit = true;
        }
        if hit {
            term_hits += 1;
        }
    }

    let mut s = kind_base(&c.sym.kind) + signal;

    // Corroboration multiplier: each extra distinct query term is a strong
    // relevance boost.
    s *= 1.0 + 0.8 * (term_hits.saturating_sub(1) as f64);

    // A lone match on a single term, when the symbol is a trivial member
    // (getter/field/local), is almost always noise — damp it hard.
    if term_hits <= 1 && matches!(c.sym.kind.as_str(), "method" | "function" | "property" | "variable") {
        s *= 0.45;
    }

    // Prefer concise names on ties (long auto-generated names rank lower).
    s -= c.sym.name.chars().count() as f64 * 0.05;

    // Penalties (multiplicative, applied last).
    if c.vendor {
        s *= 0.05; // .d.ts / node_modules — keep out of the top, never delete.
    }
    if is_test_path(&c.sym.path) {
        s *= 0.3; // tests shown in their own section, not as primary source.
    }
    if let (Some(dom), Some(ext)) = (dom_lang, ext_of(&c.sym.path)) {
        if ext != dom {
            s *= 0.4; // cross-stack down-rank.
        }
    }
    s
}

fn kind_base(kind: &str) -> f64 {
    match kind {
        "function" | "method" => 10.0,
        "class" | "interface" | "struct" | "module" | "trait" => 8.0,
        "enum" => 6.0,
        "constant" => 4.0,
        _ => 5.0,
    }
}

fn dominant_lang(cands: &[Cand]) -> Option<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in cands {
        if c.vendor {
            continue;
        }
        if let Some(ext) = ext_of(&c.sym.path) {
            *counts.entry(ext).or_insert(0) += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(e, _)| e)
}

// ---------------------------------------------------------------------------
// Tests by path convention (language-agnostic registry)
// ---------------------------------------------------------------------------

fn find_tests_by_convention(conn: &Connection, rel: &str) -> Result<Vec<String>> {
    let stem = path_stem(rel);
    let ext = ext_of(rel).unwrap_or_default();
    let mut patterns: Vec<String> = Vec::new();
    match ext.as_str() {
        "rb" => {
            patterns.push(format!("{stem}_spec.rb"));
            patterns.push(format!("{stem}_test.rb"));
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            for e in ["ts", "tsx", "js", "jsx"] {
                patterns.push(format!("{stem}.test.{e}"));
                patterns.push(format!("{stem}.spec.{e}"));
            }
        }
        "go" => patterns.push(format!("{stem}_test.go")),
        "py" => {
            patterns.push(format!("test_{stem}.py"));
            patterns.push(format!("{stem}_test.py"));
        }
        "kt" | "java" | "scala" => {
            patterns.push(format!("{stem}Test.{ext}"));
            patterns.push(format!("{stem}Spec.{ext}"));
        }
        "rs" => {
            // Rust tests are usually inline (#[cfg(test)]) — not path-detectable.
            return Ok(vec!["(rust: inline #[cfg(test)] — not path-detected)".to_string()]);
        }
        _ => {}
    }
    let mut found = Vec::new();
    for p in patterns {
        for hit in db::find_files(conn, &p, 3)? {
            if !found.contains(&hit) {
                found.push(hit);
            }
        }
    }
    Ok(found)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn emit_text(
    root: &Path,
    raw: &str,
    dom_lang: Option<&str>,
    cands: &[Cand],
    file_order: &[usize],
    tests: &[(String, Vec<String>)],
    resolver: &PathResolver,
) {
    let n_files = file_order.len();
    println!(
        "{} {}",
        "Exploration:".bold(),
        raw.bold()
    );
    if let Some(d) = dom_lang {
        println!("  dominant language: .{}  ·  {} symbols matched", d, cands.len());
    }

    println!("\n{}", "Relevant symbols:".cyan());
    for c in cands.iter().take(MAX_SYMBOLS_LISTED) {
        let disp = resolver.resolve_with_root(&c.sym.path, c.sym.root_path.as_deref());
        let tag = if c.vendor { " (vendor)".dimmed().to_string() } else { String::new() };
        println!(
            "  {} [{}]  {}:{}  {}{}",
            c.sym.display_name().cyan(),
            c.sym.kind,
            disp,
            c.sym.line,
            format!("score={:.0}", c.score).dimmed(),
            tag
        );
    }

    println!("\n{} ({} files)", "Source (from disk):".cyan(), n_files);
    for &i in file_order {
        let c = &cands[i];
        let disp = resolver.resolve_with_root(&c.sym.path, c.sym.root_path.as_deref());
        println!("\n{} {} — {}", "####".dimmed(), disp, c.sym.display_name());
        match read_snippet(root, &c.sym) {
            Some(snip) => print!("{}", snip),
            None => println!("  (could not read source)"),
        }
    }

    println!("\n{}", "Tests (by path convention):".cyan());
    for (rel, found) in tests {
        if found.is_empty() {
            println!("  {} ← {}", rel, "no test file found by convention".red());
        } else {
            println!("  {} ← {}", rel, found.join(", ").green());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_json(
    root: &Path,
    raw: &str,
    dom_lang: Option<&str>,
    cands: &[Cand],
    file_order: &[usize],
    tests: &[(String, Vec<String>)],
) -> Result<()> {
    let symbols: Vec<_> = cands
        .iter()
        .take(MAX_SYMBOLS_LISTED)
        .map(|c| {
            json!({
                "name": c.sym.display_name(),
                "kind": c.sym.kind,
                "path": c.sym.path,
                "line": c.sym.line,
                "score": c.score,
                "vendor": c.vendor,
            })
        })
        .collect();
    let files: Vec<_> = file_order
        .iter()
        .map(|&i| {
            let c = &cands[i];
            json!({
                "path": c.sym.path,
                "symbol": c.sym.display_name(),
                "line": c.sym.line,
                "source": read_snippet(root, &c.sym).unwrap_or_default(),
            })
        })
        .collect();
    let tests_json: Vec<_> = tests
        .iter()
        .map(|(rel, found)| json!({ "source": rel, "tests": found }))
        .collect();
    let out = json!({
        "query": raw,
        "dominant_language": dom_lang,
        "symbols": symbols,
        "files": files,
        "tests": tests_json,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Source extraction (from disk, by coordinates + indentation heuristic)
// ---------------------------------------------------------------------------

fn read_snippet(root: &Path, sym: &SearchResult) -> Option<String> {
    let abs = abs_path(root, &sym.path, sym.root_path.as_deref());
    let content = fs::read_to_string(&abs).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let start = (sym.line.max(1) as usize) - 1;
    if start >= lines.len() {
        return None;
    }
    let end = block_end(&lines, start);
    let mut out = String::new();
    for (n, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:>5}\t{}\n", start + n + 1, line));
    }
    Some(out)
}

/// Determine the end (exclusive) of a block starting at `start`, by indentation.
/// Language-agnostic: include the start line, following blank lines and any
/// line indented deeper than the start; stop at the first line indented at or
/// below the start (including one trailing closer like `}` / `end`). Capped.
fn block_end(lines: &[&str], start: usize) -> usize {
    let base = indent_width(lines[start]);
    let mut j = start + 1;
    while j < lines.len() && (j - start) < SNIPPET_CAP_LINES {
        let line = lines[j];
        if line.trim().is_empty() {
            j += 1;
            continue;
        }
        if indent_width(line) <= base {
            // Include a lone closing delimiter at base indent (}, end, ), ]).
            let t = line.trim();
            if matches!(t, "}" | "end" | ")" | "]" | "};" | "})" | "end;") {
                j += 1;
            }
            break;
        }
        j += 1;
    }
    j.min(lines.len())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tokenize(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for part in raw.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let t = part.trim().to_lowercase();
        if t.chars().count() >= 3 && seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

fn indent_width(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn ext_of(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
}

fn path_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

fn is_vendor(path: &str) -> bool {
    path.contains("node_modules") || path.ends_with(".d.ts")
}

fn is_test_path(path: &str) -> bool {
    let p = path.to_lowercase();
    p.contains("/spec/")
        || p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/__tests__/")
        || p.contains("_spec.")
        || p.contains("_test.")
        || p.contains(".spec.")
        || p.contains(".test.")
}

fn abs_path(root: &Path, rel: &str, root_path: Option<&str>) -> PathBuf {
    match root_path {
        Some(rp) if !rp.is_empty() => Path::new(rp).join(rel),
        _ => root.join(rel),
    }
}
