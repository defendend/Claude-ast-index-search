//! `explore` — Stage A prototype.
//!
//! One-shot context for a query: rank the most relevant symbols, show the
//! best files — the source of a function (read fresh from disk — never stored
//! in the DB), an outline of a type or module — their neighbours
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

use super::{is_test_path, PathResolver};
use crate::db::{self, SearchResult, SearchScope, SymbolSpan};

/// Candidates pulled per query term (and per compound) from FTS before ranking.
const SEED_PER_TERM: usize = 40;
/// Candidates pulled by one bm25 ranking over all query terms.
const RANKED_SEEDS: usize = 200;
/// Candidates pulled from the files whose path spells out every query term.
const PATH_SEEDS: usize = 60;
const PATH_SEEDS_PER_FILE: usize = 3;
/// Longest run of consecutive query words joined into one compound.
const MAX_COMPOUND_WORDS: usize = 4;
/// Max symbols listed in the ranked "Relevant symbols" section.
const MAX_SYMBOLS_LISTED: usize = 15;
/// Source files shown when `explore` runs as the `search` fallback; matches
/// the CLI default so both paths return the same amount of context.
const DEFAULT_MAX_FILES: usize = 6;
/// Hard cap on lines per source snippet (god-file / minified protection).
const SNIPPET_CAP_LINES: usize = 60;
/// Hard cap on outline rows shown for one definition.
const OUTLINE_CAP_ROWS: usize = 40;

/// Words of a question that name no code: "how does the merge work" asks
/// about `merge`, and `works` would otherwise match `workspace`.
const QUESTION_WORDS: &[&str] = &[
    "how", "does", "did", "what", "where", "which", "why", "when", "who", "the", "and", "for",
    "with", "from", "into", "that", "this", "are", "was", "were", "work", "works", "working",
];

/// Kinds that hold other definitions: shown as an outline, and what a query
/// naming a type or module is after.
const CONTAINER_KINDS: &[&str] = &["class", "interface", "object", "enum", "package"];

/// Kinds whose own text is the answer, shown as source rather than an outline.
const BODY_KINDS: &[&str] = &["function", "procedure", "typealias", "constant", "table"];

/// A query as `explore` reads it.
struct Query {
    /// Lowercased identifier words of 3+ characters, question words dropped.
    terms: Vec<String>,
    /// Runs of consecutive words joined together, longest first — how the
    /// full-text index tokenizes a CamelCase name (`merge service` is
    /// `mergeservice`). Short words stay in: `pdf to html` is `pdftohtml`.
    compounds: Vec<String>,
    /// Every word joined together: the name a symbol would carry if the query
    /// spelled it out exactly.
    whole: String,
}

impl Query {
    fn parse(raw: &str) -> Self {
        let words: Vec<String> = raw
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .filter(|word| !QUESTION_WORDS.contains(&word.as_str()))
            .collect();
        let mut compounds: Vec<String> = Vec::new();
        for len in (2..=words.len().min(MAX_COMPOUND_WORDS)).rev() {
            for window in words.windows(len) {
                let compound = window.concat();
                if !compounds.contains(&compound) {
                    compounds.push(compound);
                }
            }
        }
        Query {
            terms: tokenize(raw),
            compounds,
            whole: words.concat(),
        }
    }
}

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
    scope: &SearchScope,
) -> Result<()> {
    run_explore(root, query, max_files, use_rwr, format, scope, None)
}

fn run_explore(
    root: &Path,
    query: &[String],
    max_files: usize,
    use_rwr: bool,
    format: &str,
    scope: &SearchScope,
    fallback_reason: Option<&str>,
) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }

    let conn = db::open_db_leased(root)?;
    let resolver = PathResolver::try_from_conn(root, &conn)?;

    let raw = query.join(" ");
    let query = Query::parse(&raw);
    if query.terms.is_empty() {
        println!("explore: query has no usable terms (need identifiers >= 3 chars)");
        return Ok(());
    }

    // 1. Seed, then dedup by (path, line): one bm25 ranking over all terms,
    //    compound names, files whose path spells the query out, and a per-term
    //    sample with a fuzzy fallback when a term is thin.
    let mut hits = db::search_symbol_seeds_ranked(&conn, &query.terms, RANKED_SEEDS)?;
    for compound in &query.compounds {
        hits.extend(db::search_symbols(
            &conn,
            &format!("{compound}*"),
            SEED_PER_TERM,
        )?);
    }
    if query.terms.len() >= 2 {
        hits.extend(db::search_symbols_in_matching_paths(
            &conn,
            &query.terms,
            PATH_SEEDS_PER_FILE,
            PATH_SEEDS,
        )?);
    }
    for term in &query.terms {
        let term_hits = db::search_symbol_seeds(&conn, term, SEED_PER_TERM)?;
        let thin = term_hits.len() < 3;
        hits.extend(term_hits);
        if thin {
            hits.extend(db::search_symbols_fuzzy(&conn, term, SEED_PER_TERM)?);
        }
    }
    let mut cands: Vec<Cand> = Vec::new();
    let mut seen: HashSet<(String, i64)> = HashSet::new();
    for s in hits {
        if !resolver.matches_filter(s.root_path.as_deref()) {
            continue;
        }
        if !scope.matches_path(&s.path) {
            continue;
        }
        if !seen.insert((s.path.clone(), s.line)) {
            continue;
        }
        let vendor = db::is_vendor_path(&s.path);
        cands.push(Cand {
            sym: s,
            score: 0.0,
            vendor,
            link: None,
        });
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
        c.score = score(c, &query, dom_lang.as_deref());
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

    let contexts: Vec<Option<FileContext>> = file_order
        .iter()
        .map(|&i| file_context(&conn, root, &cands[i].sym))
        .collect();

    if format == "json" {
        return emit_json(
            &raw,
            dom_lang.as_deref(),
            &cands,
            &file_order,
            &contexts,
            &tests,
            fallback_reason,
        );
    }

    emit_text(
        &raw,
        dom_lang.as_deref(),
        &cands,
        &file_order,
        &contexts,
        &tests,
        &resolver,
    );
    Ok(())
}

/// Entry point used by `search` when literal matching finds nothing for a
/// multi-word query. Runs the ranking engine on the same query and labels the
/// output so the caller knows it did not get a literal match.
pub fn cmd_search_fallback(
    root: &Path,
    query: &str,
    format: &str,
    scope: &SearchScope,
) -> Result<()> {
    const REASON: &str =
        "no literal matches for a multi-word query; results are ranked by relevance";
    if format != "json" {
        println!(
            "{}",
            format!(
                "No literal matches for '{}'. Showing relevance-ranked results (same as `ast-index explore`):",
                query
            )
            .yellow()
        );
        println!();
    }
    run_explore(
        root,
        &[query.to_string()],
        DEFAULT_MAX_FILES,
        false,
        format,
        scope,
        Some(REASON),
    )
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

fn score(c: &Cand, query: &Query, dom_lang: Option<&str>) -> f64 {
    let terms = &query.terms;
    let name_lc = c.sym.name.to_lowercase();
    let own_lc = db::last_name_segment(&c.sym.name).to_lowercase();
    let qual_lc = c.sym.qualified_name.as_deref().unwrap_or("").to_lowercase();
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
        // A word in the symbol's own name says more than one only in its
        // namespace: `Applicant::MergeService` is the merge, while
        // `Applicant::Merge::CallbacksService` is one of its helpers.
        if name_lc == *t {
            signal += 50.0;
            hit = true;
        } else if own_lc.contains(t) {
            signal += 25.0;
            hit = true;
        } else if name_lc.contains(t) {
            signal += 15.0;
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
    if term_hits <= 1
        && matches!(
            c.sym.kind.as_str(),
            "method" | "function" | "property" | "variable"
        )
    {
        s *= 0.45;
    }

    // Prefer concise names on ties (long auto-generated names rank lower).
    s -= c.sym.name.chars().count() as f64 * 0.05;

    // The symbol the query spells out — `ApplicationService` for "application
    // service" — outranks the ones that merely contain its words; so, less
    // strongly, does one named by a run of them.
    let segment = flatten(db::last_name_segment(&c.sym.name));
    if !query.whole.is_empty() && (flatten(&c.sym.name) == query.whole || segment == query.whole) {
        s *= 2.0;
    } else if query.compounds.contains(&segment) {
        s *= 1.5;
    }

    if is_primary_definition(&c.sym) {
        s *= 1.3;
    }

    s * penalty_mult(&c.sym, c.vendor, dom_lang)
}

/// Whether `sym` is the type or module its file is named after —
/// `MergeService` in `merge_service.rb`, `PaymentGateway` in
/// `PaymentGateway.kt` — rather than a helper or a namespace wrapper.
fn is_primary_definition(sym: &SearchResult) -> bool {
    CONTAINER_KINDS.contains(&sym.kind.as_str())
        && flatten(db::last_name_segment(&sym.name)) == flatten(&path_stem(&sym.path))
}

/// `s` lowercased with everything but letters and digits removed, so that
/// `MergeService`, `merge_service` and `merge-service` compare equal.
fn flatten(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
    if is_declaration_statement(sym) {
        m *= 0.4;
    }
    if is_namespace_wrapper(sym) {
        m *= 0.3;
    }
    if let (Some(dom), Some(ext)) = (dom_lang, ext_of(&sym.path)) {
        if ext != dom {
            m *= 0.4; // cross-stack down-rank (e.g. JMH .java in a Kotlin repo).
        }
    }
    m
}

/// Whether `sym` is a statement the index records as a symbol — Ruby
/// `has_many :duplicates`, `scope :active`, `include Worker`, a DSL
/// `enum` — rather than a definition. Its name holds whitespace; a type
/// statement such as Rust's `impl Graph` still holds definitions and is not
/// one.
fn is_declaration_statement(sym: &SearchResult) -> bool {
    sym.name.contains(char::is_whitespace) && !CONTAINER_KINDS.contains(&sym.kind.as_str())
}

/// Whether `sym` is a module or package opened only to nest the file's real
/// definition — `module Applicant::Merge` around every class of
/// `applicant/merge/` — which repeats once per file under the same name.
fn is_namespace_wrapper(sym: &SearchResult) -> bool {
    sym.kind == "package" && !is_primary_definition(sym)
}

fn kind_base(kind: &str) -> f64 {
    match kind {
        "class" | "interface" | "object" => 12.0,
        "function" | "procedure" => 10.0,
        "enum" | "package" => 8.0,
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
            return Ok(vec![
                "(rust: inline #[cfg(test)] — not path-detected)".to_string()
            ]);
        }
        _ => {}
    }
    let mut found = Vec::new();
    for p in patterns {
        for hit in db::find_files(conn, &p, TEST_CANDIDATES_PER_PATTERN)? {
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
    Ok(closest_tests(rel, found))
}

/// Candidate test files per name pattern read before [`closest_tests`]
/// picks among them; a common file name has one per package.
const TEST_CANDIDATES_PER_PATTERN: usize = 50;

/// Directory names that hold tests rather than mirror the source tree.
const TEST_ROOT_DIRS: &[&str] = &["spec", "specs", "test", "tests", "__tests__", "src"];

/// The candidates whose directory ends like the source's. A test in the
/// source's own directory (`config_test.go`) ranks first; then the most
/// trailing directory names in common, test roots (`spec/`, `tests/`,
/// `__tests__/`, `src/`) left out: `app/services/billing/charge.rb` keeps
/// `spec/services/billing/charge_spec.rb` over `engines/x/spec/charge_spec.rb`,
/// and `src/main/java/a/b/X.java` keeps `src/test/java/a/b/XTest.java`. When
/// no candidate shares a directory — a flat `tests/`, separate `*.Tests`
/// projects — every candidate is kept.
fn closest_tests(source: &str, candidates: Vec<String>) -> Vec<String> {
    let parent = |path: &str| path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
    let dirs = |path: &str| -> Vec<String> {
        let mut dirs: Vec<String> = path.split('/').map(str::to_string).collect();
        dirs.pop();
        dirs.retain(|dir| !TEST_ROOT_DIRS.contains(&dir.as_str()));
        dirs
    };
    let source_parent = parent(source);
    let source_dirs = dirs(source);
    let shared = |candidate: &str| {
        if parent(candidate) == source_parent {
            return usize::MAX;
        }
        dirs(candidate)
            .iter()
            .rev()
            .zip(source_dirs.iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
    };
    let best = candidates.iter().map(|c| shared(c)).max().unwrap_or(0);
    if best == 0 {
        return candidates;
    }
    candidates
        .into_iter()
        .filter(|c| shared(c) == best)
        .collect()
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn emit_text(
    raw: &str,
    dom_lang: Option<&str>,
    cands: &[Cand],
    file_order: &[usize],
    contexts: &[Option<FileContext>],
    tests: &[(String, Vec<String>)],
    resolver: &PathResolver,
) {
    let n_files = file_order.len();
    println!("{} {}", "Exploration:".bold(), raw.bold());
    if let Some(d) = dom_lang {
        println!(
            "  dominant language: .{}  ·  {} symbols matched",
            d,
            cands.len()
        );
    }

    println!("\n{}", "Relevant symbols:".cyan());
    for c in cands.iter().take(MAX_SYMBOLS_LISTED) {
        let disp = resolver.resolve_with_root(&c.sym.path, c.sym.root_path.as_deref());
        let tag = if c.vendor {
            " (vendor)".dimmed().to_string()
        } else {
            String::new()
        };
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
    for (&i, context) in file_order.iter().zip(contexts) {
        let c = &cands[i];
        let disp = resolver.resolve_with_root(&c.sym.path, c.sym.root_path.as_deref());
        println!("\n{} {} — {}", "####".dimmed(), disp, c.sym.display_name());
        match context {
            Some(FileContext::Body(snip)) => print!("{}", snip),
            Some(FileContext::Outline {
                rows,
                hidden,
                focus,
            }) => {
                for row in rows {
                    let marker = if row.line == *focus { "→" } else { " " };
                    println!(
                        "  {} {} {} [{}]",
                        marker,
                        position(row).dimmed(),
                        row.name,
                        row.kind
                    );
                }
                if *hidden > 0 {
                    println!("    … {hidden} more");
                }
            }
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
    raw: &str,
    dom_lang: Option<&str>,
    cands: &[Cand],
    file_order: &[usize],
    contexts: &[Option<FileContext>],
    tests: &[(String, Vec<String>)],
    fallback_reason: Option<&str>,
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
        .zip(contexts)
        .map(|(&i, context)| {
            let c = &cands[i];
            let mut file = json!({
                "path": c.sym.path,
                "symbol": c.sym.display_name(),
                "line": c.sym.line,
            });
            match context {
                Some(FileContext::Outline { rows, hidden, .. }) => {
                    let rows: Vec<_> = rows
                        .iter()
                        .map(|row| {
                            json!({
                                "name": row.name,
                                "kind": row.kind,
                                "line": row.line,
                                "end_line": row.end_line,
                            })
                        })
                        .collect();
                    file["outline"] = json!(rows);
                    file["outline_hidden"] = json!(hidden);
                }
                Some(FileContext::Body(source)) => file["source"] = json!(source),
                None => file["source"] = json!(""),
            }
            file
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
    let mut out = json!({
        "query": raw,
        "dominant_language": dom_lang,
        "symbols": symbols,
        "neighbours": neighbours,
        "files": files,
        "tests": tests_json,
    });
    if let Some(reason) = fallback_reason {
        // Same document, not a second one: a `search` caller parsing the
        // response must see one valid JSON object with an explicit marker.
        out["fallback"] = json!("explore");
        out["reason"] = json!(reason);
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Source extraction (from disk, by coordinates + indentation heuristic)
// ---------------------------------------------------------------------------

/// What `explore` shows of a chosen file.
enum FileContext {
    /// Source of a function-like symbol: its body is the answer.
    Body(String),
    /// Definitions inside the type or module around the symbol, with line
    /// ranges: a few lines of a class or of a `has_many` explain nothing,
    /// while the member list says where to read next.
    Outline {
        rows: Vec<SymbolSpan>,
        /// Rows past [`OUTLINE_CAP_ROWS`] left out.
        hidden: usize,
        /// Line of the chosen symbol.
        focus: i64,
    },
}

fn position(span: &SymbolSpan) -> String {
    match span.end_line {
        Some(end) if end > span.line => format!(":{}-{}", span.line, end),
        _ => format!(":{}", span.line),
    }
}

fn file_context(conn: &Connection, root: &Path, sym: &SearchResult) -> Option<FileContext> {
    if !BODY_KINDS.contains(&sym.kind.as_str()) {
        if let Some(outline) = read_outline(conn, sym) {
            return Some(outline);
        }
    }
    read_snippet(root, sym).map(FileContext::Body)
}

/// Outline of the innermost type or module holding `sym` (`sym` itself when
/// it is one), or of the whole file when nothing holds it. Read from the
/// index, which is where `sym` and its line come from; `None` when the file
/// has no ranges to outline.
fn read_outline(conn: &Connection, sym: &SearchResult) -> Option<FileContext> {
    let spans = db::get_file_outline(conn, sym.root_path.as_deref(), &sym.path).ok()?;
    if spans.iter().all(|span| span.end_line.is_none()) {
        return None;
    }
    let focus = sym.line;
    let holder = spans
        .iter()
        .filter(|span| CONTAINER_KINDS.contains(&span.kind.as_str()))
        .filter_map(|span| Some((span.line, span.end_line?)))
        .filter(|&(start, end)| start <= focus && focus <= end)
        .min_by_key(|&(start, end)| end - start);
    let mut rows: Vec<SymbolSpan> = spans
        .into_iter()
        .filter(|span| holder.is_none_or(|(start, end)| start <= span.line && span.line <= end))
        .collect();
    let hidden = rows.len().saturating_sub(OUTLINE_CAP_ROWS);
    rows.truncate(OUTLINE_CAP_ROWS);
    Some(FileContext::Outline {
        rows,
        hidden,
        focus,
    })
}

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

/// Query terms: lowercased identifiers of 3+ characters, deduplicated, without
/// [`QUESTION_WORDS`] — unless those are all the query has.
fn tokenize(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for part in raw.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let t = part.trim().to_lowercase();
        if t.chars().count() >= 3 && seen.insert(t.clone()) {
            out.push(t);
        }
    }
    let meaningful: Vec<String> = out
        .iter()
        .filter(|t| !QUESTION_WORDS.contains(&t.as_str()))
        .cloned()
        .collect();
    if meaningful.is_empty() {
        out
    } else {
        meaningful
    }
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

    // Build edges around each seed: callers + inheritance. Callers come from
    // the symbol graph when it is built and fresh — resolved edges point at
    // this definition, not at every symbol sharing its name — and otherwise
    // from references matched by name, each attributed to its owning symbol.
    let seeds: Vec<SearchResult> = cands.iter().take(seed_n).map(|c| c.sym.clone()).collect();
    // Role a node plays relative to the seed — for the "Graph neighbours" section.
    let mut link_role: HashMap<(String, i64), &'static str> = HashMap::new();
    let graph_dependents = super::graph::resolved_dependents_of(conn, &seeds, REF_LIMIT)?;
    for (i, sym) in seeds.iter().enumerate() {
        let sid = g.intern(sym);
        // A seed the graph resolves no edge to — calls through a receiver of
        // unknown type in Java, Swift, Go — keeps the name-matched callers.
        let resolved = graph_dependents
            .as_ref()
            .map(|dependents| dependents[i].clone())
            .filter(|dependents| !dependents.is_empty());
        let callers: Vec<SearchResult> = match resolved {
            Some(dependents) => dependents,
            None => {
                let mut owners = Vec::new();
                for r in db::find_references(conn, &sym.name, REF_LIMIT)? {
                    if let Some(owner) =
                        db::find_owning_symbol(conn, r.root_path.as_deref(), &r.path, r.line)
                            .unwrap_or(None)
                    {
                        owners.push(owner);
                    }
                }
                owners
            }
        };
        let children = db::find_implementations(conn, &sym.name, REF_LIMIT)?;
        // Inheritance is also a graph edge; name it `subclass` rather than
        // `caller` when both sources list the same definition.
        let subclass_keys: HashSet<(String, i64)> =
            children.iter().map(|c| (c.path.clone(), c.line)).collect();
        for caller in callers {
            if !resolver.matches_filter(caller.root_path.as_deref()) {
                continue;
            }
            let key = (caller.path.clone(), caller.line);
            let role = if subclass_keys.contains(&key) {
                "subclass"
            } else {
                "caller"
            };
            link_role.entry(key).or_insert(role);
            let oid = g.intern(&caller);
            g.edge(sid, oid);
        }
        for child in children {
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
            let vendor = db::is_vendor_path(&sym.path);
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
        let vendor = db::is_vendor_path(path);
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
    fn test_path_detection() {
        assert!(is_test_path(
            "spec/services/applicant/merge_service_spec.rb"
        ));
        assert!(is_test_path("src/foo.test.ts"));
        assert!(is_test_path("pkg/foo_test.go"));
        assert!(is_test_path("Tests/SessionTests.swift"));
        assert!(!is_test_path("app/services/applicant/merge_service.rb"));
    }

    #[test]
    fn penalties_downrank_noise_not_real_code() {
        let real = penalty_mult(
            &sym("MergeService", "class", "app/x.rb", 1),
            false,
            Some("rb"),
        );
        let imp = penalty_mult(&sym("foo", "import", "app/x.rb", 1), false, Some("rb"));
        let vendor = penalty_mult(
            &sym("X", "interface", "node_modules/x.d.ts", 1),
            true,
            Some("rb"),
        );
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
        let terms = Query::parse("applicant merge MergeService");
        let dom = Some("rb");
        // Matches all three terms (name + path).
        let strong = score(
            &cand(
                "MergeService",
                "class",
                "app/services/applicant/merge_service.rb",
            ),
            &terms,
            dom,
        );
        // A trivial getter matching only "applicant".
        let weak = score(
            &cand(
                "applicant",
                "method",
                "app/contexts/security_form_context.rb",
            ),
            &terms,
            dom,
        );
        assert!(
            strong > weak,
            "multi-term symbol ({strong}) must outrank single-term getter ({weak})"
        );
    }

    #[test]
    fn question_words_are_dropped_unless_nothing_else_is_left() {
        assert_eq!(
            tokenize("how does applicant merge work"),
            vec!["applicant", "merge"]
        );
        let query = Query::parse("how does the merge work");
        assert_eq!(query.terms, vec!["merge"]);
        assert_eq!(query.whole, "merge");
        assert_eq!(tokenize("how does it work"), vec!["how", "does", "work"]);
    }

    #[test]
    fn compounds_join_consecutive_words_longest_first() {
        let query = Query::parse("pdf to html service");
        assert_eq!(query.terms, vec!["pdf", "html", "service"]);
        assert_eq!(query.whole, "pdftohtmlservice");
        assert_eq!(query.compounds[0], "pdftohtmlservice");
        assert!(query.compounds.contains(&"htmlservice".to_string()));
        assert!(query.compounds.contains(&"pdfto".to_string()));
        assert!(Query::parse("merge").compounds.is_empty());
        assert_eq!(Query::parse("merge_service").whole, "mergeservice");
    }

    #[test]
    fn symbol_the_query_spells_out_outranks_ones_sharing_a_word() {
        let query = Query::parse("application service");
        let dom = Some("rb");
        let named = score(
            &cand(
                "ApplicationService",
                "class",
                "app/services/application_service.rb",
            ),
            &query,
            dom,
        );
        let sharing = score(
            &cand(
                "Integrations::Application::FindAdapter",
                "class",
                "app/adapters/integrations/application/find_adapter.rb",
            ),
            &query,
            dom,
        );
        let method = score(
            &cand(
                "application_service",
                "function",
                "app/services/offer/pdf_service.rb",
            ),
            &query,
            dom,
        );
        assert!(named > sharing, "{named} vs {sharing}");
        assert!(named > method, "{named} vs {method}");
    }

    #[test]
    fn primary_definition_of_a_file_is_recognised_across_naming_styles() {
        assert!(is_primary_definition(&sym(
            "Applicant::MergeService",
            "class",
            "app/services/applicant/merge_service.rb",
            3
        )));
        assert!(is_primary_definition(&sym(
            "PaymentGateway",
            "class",
            "src/pay/PaymentGateway.kt",
            1
        )));
        assert!(!is_primary_definition(&sym(
            "Integrations::Application",
            "package",
            "app/adapters/integrations/application/find_adapter.rb",
            4
        )));
        assert!(!is_primary_definition(&sym(
            "merge_service",
            "function",
            "app/services/merge_service.rb",
            9
        )));
    }

    #[test]
    fn declaration_statements_rank_below_definitions() {
        let query = Query::parse("applicant merge");
        let dom = Some("rb");
        let path = "app/models/applicant/deduplication.rb";
        let association = score(
            &cand("has_many :applicant_merges", "property", path),
            &query,
            dom,
        );
        let method = score(&cand("applicant_merge", "function", path), &query, dom);
        assert!(method > association, "{method} vs {association}");
        assert!(!is_declaration_statement(&sym(
            "impl Graph",
            "class",
            "src/g.rs",
            1
        )));

        let merge = Query::parse("applicant merge");
        let wrapper = score(
            &cand(
                "Applicant::Merge",
                "package",
                "app/services/applicant/merge/create_event_service.rb",
            ),
            &merge,
            dom,
        );
        let service = score(
            &cand(
                "Applicant::MergeService",
                "class",
                "app/services/applicant/merge_service.rb",
            ),
            &merge,
            dom,
        );
        assert!(service > wrapper, "{service} vs {wrapper}");
        assert!(is_declaration_statement(&sym(
            "include Sidekiq::Worker",
            "annotation",
            "app/workers/w.rb",
            2
        )));
    }

    #[test]
    fn closest_tests_prefer_the_mirrored_directory() {
        let pick = |source: &str, candidates: &[&str]| {
            closest_tests(source, candidates.iter().map(|c| c.to_string()).collect())
        };
        assert_eq!(
            pick(
                "app/services/billing/charge.rb",
                &[
                    "engines/x/spec/charge_spec.rb",
                    "spec/services/billing/charge_spec.rb",
                    "spec/services/charge_spec.rb",
                ]
            ),
            vec!["spec/services/billing/charge_spec.rb"]
        );
        assert_eq!(
            pick(
                "src/main/java/a/b/X.java",
                &["src/test/java/a/b/XTest.java", "src/test/java/c/XTest.java"]
            ),
            vec!["src/test/java/a/b/XTest.java"]
        );
        assert_eq!(
            pick("src/ui/Button.tsx", &["src/ui/__tests__/Button.test.tsx"]),
            vec!["src/ui/__tests__/Button.test.tsx"]
        );
        assert_eq!(
            pick("pkg/core/parser.py", &["tests/test_parser.py"]),
            vec!["tests/test_parser.py"]
        );
        assert_eq!(
            pick("config.go", &["cmd/tool/config_test.go", "config_test.go"]),
            vec!["config_test.go"]
        );
        assert_eq!(
            pick(
                "pkg/lexer.py",
                &[
                    "tests/unit/test_lexer.py",
                    "tests/integration/test_lexer.py"
                ]
            )
            .len(),
            2
        );
        assert_eq!(
            pick(
                "src/App/InvoiceCalculator.cs",
                &[
                    "tests/App.UnitTests/InvoiceCalculatorTests.cs",
                    "tests/App.IntegrationTests/InvoiceCalculatorTests.cs",
                ]
            )
            .len(),
            2
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
