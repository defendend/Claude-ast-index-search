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

use std::collections::{HashMap, HashSet};
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
    /// How this symbol entered the graph in Stage B: "caller" (references the
    /// seed) or "subclass" (inherits from it). `None` for lexical matches.
    link: Option<&'static str>,
}

pub fn cmd_explore(
    root: &Path,
    query: &[String],
    max_files: usize,
    use_rwr: bool,
    format: &str,
) -> Result<()> {
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
                link: None,
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

    // Stage B: re-rank by RWR over an in-memory call/inheritance graph.
    if use_rwr {
        apply_rwr(&conn, &resolver, dom_lang.as_deref(), &mut cands)?;
    }

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

    s * penalty_mult(&c.sym, c.vendor, dom_lang)
}

/// Multiplicative down-ranking shared by the lexical pass (Stage A) and the
/// RWR blend (Stage B), so penalties are not lost when graph re-ranking runs.
fn penalty_mult(sym: &SearchResult, vendor: bool, dom_lang: Option<&str>) -> f64 {
    let mut m = 1.0;
    if vendor {
        m *= 0.05; // .d.ts / node_modules — keep out of the top, never delete.
    }
    if sym.kind == "import" {
        m *= 0.15; // import lines must not outrank real definitions.
    }
    if is_test_path(&sym.path) {
        m *= 0.3; // tests live in their own section, not as primary source.
    }
    if let (Some(dom), Some(ext)) = (dom_lang, ext_of(&sym.path)) {
        if ext != dom {
            m *= 0.4; // cross-stack down-rank (e.g. JMH .java in a Kotlin repo).
        }
    }
    m
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
        // Vue/Svelte components are tested with JS/TS test files next to them
        // or under __tests__ (X.spec.ts / X.test.ts / X.spec.js …).
        "vue" | "svelte" => {
            for e in ["ts", "js", "tsx", "jsx"] {
                patterns.push(format!("{stem}.spec.{e}"));
                patterns.push(format!("{stem}.test.{e}"));
            }
        }
        "go" => patterns.push(format!("{stem}_test.go")),
        "py" => {
            patterns.push(format!("test_{stem}.py"));
            patterns.push(format!("{stem}_test.py"));
        }
        // JVM family: both singular `Test`/`Spec` and plural `Tests` are common.
        "kt" | "java" | "scala" => {
            patterns.push(format!("{stem}Test.{ext}"));
            patterns.push(format!("{stem}Tests.{ext}"));
            patterns.push(format!("{stem}Spec.{ext}"));
        }
        // Swift / XCTest convention is plural `XTests.swift`.
        "swift" => {
            patterns.push(format!("{stem}Tests.swift"));
            patterns.push(format!("{stem}Test.swift"));
            patterns.push(format!("{stem}Spec.swift"));
        }
        // C#: NUnit/xUnit use `XTests.cs` / `XTest.cs`.
        "cs" => {
            patterns.push(format!("{stem}Tests.cs"));
            patterns.push(format!("{stem}Test.cs"));
        }
        // PHP / PHPUnit: `XTest.php` (usually under tests/).
        "php" => {
            patterns.push(format!("{stem}Test.php"));
            patterns.push(format!("{stem}_test.php"));
        }
        // C / C++: no single standard — probe the common ones.
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" => {
            for e in ["cpp", "cc", "cxx", "c"] {
                patterns.push(format!("{stem}_test.{e}"));
                patterns.push(format!("test_{stem}.{e}"));
                patterns.push(format!("{stem}_tests.{e}"));
            }
        }
        "rs" => {
            // Rust tests are usually inline (#[cfg(test)]) — not path-detectable.
            return Ok(vec!["(rust: inline #[cfg(test)] — not path-detected)".to_string()]);
        }
        _ => {}
    }
    let mut found = Vec::new();
    for p in patterns {
        for hit in db::find_files(conn, &p, 5)? {
            // find_files matches `%p%` (substring), so `JsonConverter.cs` would
            // falsely match `GenericJsonConverterTests.cs`. Keep only exact
            // basename matches.
            let base = Path::new(&hit)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if base == p && !found.contains(&hit) {
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

    let neighbours: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.link.is_some() && !c.vendor)
        .collect();
    if !neighbours.is_empty() {
        println!(
            "\n{}",
            "Graph neighbours (callers / subclasses via graph):".cyan()
        );
        for c in neighbours.iter().take(10) {
            let disp = resolver.resolve_with_root(&c.sym.path, c.sym.root_path.as_deref());
            println!(
                "  [{}] {} [{}]  {}:{}",
                c.link.unwrap_or("?").magenta(),
                c.sym.display_name(),
                c.sym.kind,
                disp,
                c.sym.line
            );
        }
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
    let neighbours: Vec<_> = cands
        .iter()
        .filter(|c| c.link.is_some() && !c.vendor)
        .take(10)
        .map(|c| {
            json!({
                "link": c.link,
                "name": c.sym.display_name(),
                "kind": c.sym.kind,
                "path": c.sym.path,
                "line": c.sym.line,
            })
        })
        .collect();
    let out = json!({
        "query": raw,
        "dominant_language": dom_lang,
        "symbols": symbols,
        "neighbours": neighbours,
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
    // Vue/Svelte single-file components are whole files, not brace/indent
    // blocks — the synthetic component symbol sits at line 1, so show an
    // overview window (script + start of template) instead of a misfired
    // brace/indent slice.
    let end = match ext_of(&sym.path).as_deref() {
        Some("vue") | Some("svelte") => (start + 30).min(lines.len()),
        _ => block_end(&lines, start),
    };
    let mut out = String::new();
    for (n, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:>5}\t{}\n", start + n + 1, line));
    }
    Some(out)
}

/// Determine the end (exclusive) of a block starting at `start`.
///
/// Hybrid, language-agnostic: brace-delimited languages (C/C++/C#/Java/JS/TS/
/// Go/Rust/Swift/Kotlin/PHP) close on `{`/`}` balance — this captures full
/// method bodies that the pure-indentation heuristic missed (it returned only
/// the signature when the body opened with a brace). Indentation-delimited
/// languages (Python/Ruby) fall back to the indent rule. Capped either way.
fn block_end(lines: &[&str], start: usize) -> usize {
    let cap = (start + SNIPPET_CAP_LINES).min(lines.len());
    // Brace-style if an opening `{` appears on the signature lines — covers
    // both `fn f() {` and the Allman `fn f()\n{`.
    let probe = (start + 4).min(lines.len());
    let brace_style = lines[start..probe].iter().any(|l| l.contains('{'));
    if brace_style {
        return brace_block_end(lines, start, cap);
    }
    indent_block_end(lines, start, cap)
}

/// End of a brace-delimited block: first line where `{`/`}` balance returns to
/// zero after the opening brace. Ignores `//` line comments; string/`/* */`
/// edge cases are tolerated (rare in a signature+body window).
fn brace_block_end(lines: &[&str], start: usize, cap: usize) -> usize {
    let mut depth: i32 = 0;
    let mut opened = false;
    let mut j = start;
    while j < cap {
        let code = match lines[j].find("//") {
            Some(i) => &lines[j][..i],
            None => lines[j],
        };
        for ch in code.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        j += 1;
        if opened && depth <= 0 {
            return j;
        }
    }
    j
}

/// End of an indentation-delimited block: blank lines and any line indented
/// deeper than the start are included; stop at the first line at/below the
/// start indent, keeping a lone trailing closer (`}` / `end`).
fn indent_block_end(lines: &[&str], start: usize, cap: usize) -> usize {
    let base = indent_width(lines[start]);
    let mut j = start + 1;
    while j < cap {
        let line = lines[j];
        if line.trim().is_empty() {
            j += 1;
            continue;
        }
        if indent_width(line) <= base {
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
    let dir_or_infix = p.contains("/spec/")
        || p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/__tests__/")
        || p.starts_with("spec/")
        || p.starts_with("test/")
        || p.starts_with("tests/")
        || p.contains("_spec.")
        || p.contains("_test.")
        || p.contains(".spec.")
        || p.contains(".test.");
    if dir_or_infix {
        return true;
    }
    // CamelCase filename suffix: FooTest.java, SessionTests.swift, BarSpec.kt.
    // Use original case so plain words ("latest", "contest") are not flagged.
    let stem = path_stem(path);
    stem.ends_with("Test") || stem.ends_with("Tests") || stem.ends_with("Spec")
}

fn abs_path(root: &Path, rel: &str, root_path: Option<&str>) -> PathBuf {
    match root_path {
        Some(rp) if !rp.is_empty() => Path::new(rp).join(rel),
        _ => root.join(rel),
    }
}

// ---------------------------------------------------------------------------
// Stage B: graph + RWR (personalized PageRank)
// ---------------------------------------------------------------------------

/// In-memory undirected graph of symbols, keyed by (path, line).
struct Graph {
    idx: HashMap<(String, i64), usize>,
    syms: Vec<SearchResult>,
    restart: Vec<f64>,
    adj: Vec<HashSet<usize>>,
}

impl Graph {
    fn new() -> Self {
        Graph {
            idx: HashMap::new(),
            syms: Vec::new(),
            restart: Vec::new(),
            adj: Vec::new(),
        }
    }

    fn intern(&mut self, s: &SearchResult) -> usize {
        let key = (s.path.clone(), s.line);
        if let Some(&i) = self.idx.get(&key) {
            return i;
        }
        let i = self.syms.len();
        self.idx.insert(key, i);
        self.syms.push(s.clone());
        self.restart.push(0.0);
        self.adj.push(HashSet::new());
        i
    }

    fn edge(&mut self, a: usize, b: usize) {
        if a != b {
            self.adj[a].insert(b);
            self.adj[b].insert(a);
        }
    }
}

/// Re-rank candidates by personalized PageRank over a call/inheritance graph
/// built on the fly around the lexical seed. Restart vector = lexical scores;
/// edges = callers (each reference attributed to its owning symbol) and
/// inheritance. This surfaces structurally-central symbols the lexical pass
/// ranked low and demotes lexical matches that connect to nothing — the role
/// RWR plays in codegraph, but here on a graph assembled at query time
/// (no schema migration, no parser changes).
fn apply_rwr(
    conn: &Connection,
    resolver: &PathResolver,
    dom_lang: Option<&str>,
    cands: &mut Vec<Cand>,
) -> Result<()> {
    const SEED_NODES: usize = 30;
    const REF_LIMIT: usize = 40;
    const ALPHA: f64 = 0.25;
    const ITERS: usize = 25;

    let seed_n = SEED_NODES.min(cands.len());
    if seed_n == 0 {
        return Ok(());
    }

    let mut g = Graph::new();
    for c in cands.iter().take(seed_n) {
        let id = g.intern(&c.sym);
        g.restart[id] += c.score.max(0.0);
    }

    // Build edges around each seed: callers (refs → owning symbol) + inheritance.
    let seeds: Vec<SearchResult> = cands.iter().take(seed_n).map(|c| c.sym.clone()).collect();
    let mut file_cache: HashMap<String, Vec<SearchResult>> = HashMap::new();
    // Role a node plays relative to the seed — for the "Graph neighbours" section.
    let mut link_role: HashMap<(String, i64), &'static str> = HashMap::new();
    for sym in &seeds {
        let sid = g.intern(sym);
        for r in db::find_references(conn, &sym.name, REF_LIMIT)? {
            if !resolver.matches_filter(r.root_path.as_deref()) {
                continue;
            }
            let fsyms = file_cache
                .entry(r.path.clone())
                .or_insert_with(|| db::get_file_symbols(conn, &r.path).unwrap_or_default());
            if let Some(owner) = fsyms.iter().rev().find(|s| s.line <= r.line).cloned() {
                link_role
                    .entry((owner.path.clone(), owner.line))
                    .or_insert("caller");
                let oid = g.intern(&owner);
                g.edge(sid, oid);
            }
        }
        for child in db::find_implementations(conn, &sym.name, REF_LIMIT)? {
            if !resolver.matches_filter(child.root_path.as_deref()) {
                continue;
            }
            link_role
                .entry((child.path.clone(), child.line))
                .or_insert("subclass");
            let cid = g.intern(&child);
            g.edge(sid, cid);
        }
    }

    let n = g.syms.len();
    if n == 0 {
        return Ok(());
    }

    // Normalize restart to a probability distribution.
    let sum: f64 = g.restart.iter().sum();
    if sum > 0.0 {
        for x in g.restart.iter_mut() {
            *x /= sum;
        }
    } else {
        for x in g.restart.iter_mut() {
            *x = 1.0 / n as f64;
        }
    }

    // Power iteration. Dangling nodes (no edges) redistribute via the restart vector.
    let mut s = g.restart.clone();
    for _ in 0..ITERS {
        let mut next = vec![0.0; n];
        for i in 0..n {
            let deg = g.adj[i].len();
            if deg == 0 {
                for (j, nx) in next.iter_mut().enumerate() {
                    *nx += s[i] * g.restart[j];
                }
            } else {
                let share = s[i] / deg as f64;
                for &j in &g.adj[i] {
                    next[j] += share;
                }
            }
        }
        for i in 0..n {
            s[i] = (1.0 - ALPHA) * next[i] + ALPHA * g.restart[i];
        }
    }

    // Blend normalized RWR mass with normalized lexical score. Lexical is
    // weighted higher so an exact query hit stays near the top, while RWR
    // lifts structurally-connected symbols (callers, subclasses) the lexical
    // pass ranked low or missed entirely. Unconnected lexical-only matches are
    // halved so graph-relevant results win ties.
    let max_rwr = s.iter().cloned().fold(0.0_f64, f64::max).max(1e-9);
    let max_lex = cands
        .iter()
        .map(|c| c.score.max(0.0))
        .fold(0.0_f64, f64::max)
        .max(1e-9);

    let mut rwr_by_key: HashMap<(String, i64), f64> = HashMap::new();
    for (i, sym) in g.syms.iter().enumerate() {
        rwr_by_key.insert((sym.path.clone(), sym.line), s[i]);
    }

    // Surface graph-discovered neighbours the lexical pass never produced.
    let existing: HashSet<(String, i64)> = cands
        .iter()
        .map(|c| (c.sym.path.clone(), c.sym.line))
        .collect();
    for sym in &g.syms {
        let key = (sym.path.clone(), sym.line);
        if !existing.contains(&key) {
            let vendor = is_vendor(&sym.path);
            let link = link_role.get(&key).copied();
            cands.push(Cand {
                sym: sym.clone(),
                score: 0.0,
                vendor,
                link,
            });
        }
    }

    for c in cands.iter_mut() {
        let key = (c.sym.path.clone(), c.sym.line);
        let lex = c.score.max(0.0) / max_lex;
        let rwr = rwr_by_key.get(&key).copied().unwrap_or(0.0) / max_rwr;
        let connected = rwr_by_key.contains_key(&key);
        let blended = 0.6 * lex + 0.4 * rwr;
        c.score = blended
            * 1000.0
            * if connected { 1.0 } else { 0.5 }
            * penalty_mult(&c.sym, c.vendor, dom_lang);
    }
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str, kind: &str, path: &str, line: i64) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            qualified_name: None,
            kind: kind.to_string(),
            line,
            signature: None,
            path: path.to_string(),
            root_path: None,
        }
    }

    fn cand(name: &str, kind: &str, path: &str) -> Cand {
        let vendor = is_vendor(path);
        Cand {
            sym: sym(name, kind, path, 1),
            score: 0.0,
            vendor,
            link: None,
        }
    }

    #[test]
    fn tokenize_splits_filters_and_dedups() {
        let t = tokenize("applicant merge MergeService a, of");
        assert_eq!(t, vec!["applicant", "merge", "mergeservice"]);
        // "a"/"of" dropped (<3 chars); dedup keeps first occurrence.
        assert_eq!(tokenize("Foo foo FOO"), vec!["foo"]);
    }

    #[test]
    fn vendor_detection() {
        assert!(is_vendor("node_modules/@types/react/index.d.ts"));
        assert!(is_vendor("frontend/types/global.d.ts"));
        assert!(!is_vendor("app/services/applicant/merge_service.rb"));
    }

    #[test]
    fn test_path_detection() {
        assert!(is_test_path("spec/services/applicant/merge_service_spec.rb"));
        assert!(is_test_path("src/foo.test.ts"));
        assert!(is_test_path("pkg/foo_test.go"));
        assert!(is_test_path("Tests/SessionTests.swift"));
        assert!(!is_test_path("app/services/applicant/merge_service.rb"));
    }

    #[test]
    fn penalties_downrank_noise_not_real_code() {
        let real = penalty_mult(&sym("MergeService", "class", "app/x.rb", 1), false, Some("rb"));
        let imp = penalty_mult(&sym("foo", "import", "app/x.rb", 1), false, Some("rb"));
        let vendor = penalty_mult(&sym("X", "interface", "node_modules/x.d.ts", 1), true, Some("rb"));
        let test = penalty_mult(&sym("X", "class", "spec/x_spec.rb", 1), false, Some("rb"));
        let cross = penalty_mult(&sym("X", "class", "bench/x.java", 1), false, Some("kt"));
        assert_eq!(real, 1.0);
        assert!(imp < real && imp > vendor);
        assert!(vendor < 0.1);
        assert!(test < real);
        assert!(cross < real);
    }

    #[test]
    fn corroboration_beats_single_common_term() {
        let terms = vec![
            "applicant".to_string(),
            "merge".to_string(),
            "mergeservice".to_string(),
        ];
        let dom = Some("rb");
        // Matches all three terms (name + path).
        let strong = score(
            &cand("MergeService", "class", "app/services/applicant/merge_service.rb"),
            &terms,
            dom,
        );
        // A trivial getter matching only "applicant".
        let weak = score(
            &cand("applicant", "method", "app/contexts/security_form_context.rb"),
            &terms,
            dom,
        );
        assert!(
            strong > weak,
            "multi-term symbol ({strong}) must outrank single-term getter ({weak})"
        );
    }

    #[test]
    fn block_end_stops_at_dedent_and_keeps_closer() {
        let lines = vec!["def foo", "  body1", "  body2", "end", "def bar"];
        // Includes def..end (closer at base indent kept), stops before `def bar`.
        assert_eq!(block_end(&lines, 0), 4);
    }

    #[test]
    fn block_end_respects_cap() {
        let mut lines = vec!["def big"];
        let deep: Vec<String> = (0..200).map(|i| format!("  line{i}")).collect();
        for l in &deep {
            lines.push(l);
        }
        assert!(block_end(&lines, 0) - 0 <= SNIPPET_CAP_LINES);
    }

    #[test]
    fn block_end_brace_balanced_captures_full_body() {
        let lines = vec![
            "func f() {",     // 0
            "  if (x) {",     // 1
            "    g();",       // 2
            "  }",            // 3
            "}",              // 4
            "func next() {}", // 5
        ];
        // Brace balance closes at line 4 → end (exclusive) = 5, full body, not
        // just the signature.
        assert_eq!(block_end(&lines, 0), 5);
    }

    #[test]
    fn ext_and_stem_helpers() {
        assert_eq!(ext_of("app/x/merge_service.rb").as_deref(), Some("rb"));
        assert_eq!(ext_of("noext"), None);
        assert_eq!(path_stem("app/x/merge_service.rb"), "merge_service");
    }
}
