//! Symbol-to-symbol dependency graph and the `graph` commands built on it.
//!
//! `graph build` turns the name-based `refs` table into directed edges
//! "symbol that contains the reference -> definition the reference names".
//! The source side is exact: it is the owning symbol by line range, the same
//! rule `db::find_owning_symbol` applies. The target side is only a name, and
//! one name routinely has hundreds of definitions in a monorepo, so every edge
//! carries the level at which its target was resolved:
//!
//! * `local`     — defined in the same file, innermost enclosing scope first;
//! * `scoped`    — pinned by an explicit namespace (`A::B::Name`), by lexical
//!   nesting, by a constant receiver (`Type.method`) or by the inheritance /
//!   mixin chain of the enclosing class;
//! * `import`    — bound by an import of the source file (JavaScript and
//!   TypeScript imports and Rust `use` declarations are read from the source
//!   at build time);
//! * `unique`    — the only definition of the name in the language, for a
//!   plain reference in a language where that is meaningful evidence;
//! * `ambiguous` — `candidates` definitions share the name and nothing above
//!   narrowed them down, or the reference cannot be pinned at all (a call on
//!   a receiver of unknown type, a Ruby call on implicit `self` that the
//!   class hierarchy does not define), so even one candidate is a guess.
//!
//! Ambiguous references are stored as one edge per candidate (up to
//! [`AMBIGUITY_CAP`]) so "who might depend on this" stays answerable, but
//! every metric — fan-in/out, PageRank, dependents — counts resolved edges
//! only and reports the ambiguous ones in separate counters.
//!
//! Building is explicit (`graph build`) so `rebuild` and `update` stay as
//! fast as before. The build records the index's write generation and highest
//! row ids (`db::index_fingerprint`); every query compares them with the live
//! index and flags a stale graph instead of answering from outdated edges
//! silently.

mod metrics;
mod resolve;
mod rust;
mod schema;

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::{bail, Result};
use colored::Colorize;
use rusqlite::Connection;
use serde::Serialize;

use super::{is_test_path, Page, PathResolver};
use crate::db::{self, GraphSymbolInfo, SymbolEdgeRow, SymbolGraphMetrics};
use crate::parsers::FileType;

pub use resolve::{
    build_symbol_graph, ConfidenceCount, DropCount, DropReason, GraphBuildSummary, SchemaSummary,
};
pub use schema::SchemaLinkSummary;

/// References whose name matches more definitions than this are not stored
/// at all: an edge to each of 681 `call` methods is noise, not information.
pub const AMBIGUITY_CAP: usize = 8;
/// Depth of the precomputed `dependents` metric (transitive dependents over
/// resolved edges). Deeper impact is available on demand via `graph impact`.
pub const DEPENDENTS_DEPTH: usize = 3;

// ---------------------------------------------------------------------------
// Confidence levels
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Local,
    Scoped,
    Import,
    Unique,
    Ambiguous,
}

impl Confidence {
    pub const ALL: [Confidence; 5] = [
        Confidence::Local,
        Confidence::Scoped,
        Confidence::Import,
        Confidence::Unique,
        Confidence::Ambiguous,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Confidence {
        match code {
            0 => Confidence::Local,
            1 => Confidence::Scoped,
            2 => Confidence::Import,
            3 => Confidence::Unique,
            _ => Confidence::Ambiguous,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Local => "local",
            Confidence::Scoped => "scoped",
            Confidence::Import => "import",
            Confidence::Unique => "unique",
            Confidence::Ambiguous => "ambiguous",
        }
    }

    pub fn is_resolved(self) -> bool {
        self != Confidence::Ambiguous
    }
}

/// Highest confidence code a query should follow.
fn max_confidence(include_ambiguous: bool) -> u8 {
    if include_ambiguous {
        Confidence::Ambiguous.code()
    } else {
        Confidence::Unique.code()
    }
}

// ---------------------------------------------------------------------------
// Symbol and path classification
// ---------------------------------------------------------------------------

/// Imports and annotations (`include Foo`, decorators) are lines inside a
/// definition, not definitions: references on them belong to the enclosing
/// symbol and they are never edge targets. The same rule picks the owner of
/// a line everywhere else ([`db::is_owner_kind`]).
fn is_node_kind(kind: &str) -> bool {
    db::is_owner_kind(kind)
}

fn is_container_kind(kind: &str) -> bool {
    matches!(kind, "class" | "interface" | "object" | "enum" | "package")
}

/// Tables and columns of a database schema dump (Rails `db/schema.rb`).
fn is_schema_kind(kind: &str) -> bool {
    matches!(kind, "table" | "column")
}

/// Languages that can reference each other's definitions. A Ruby constant
/// never resolves to a TypeScript class of the same name.
fn language_family(path: &str) -> &'static str {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, ext)) = file_name.rsplit_once('.') else {
        return "other";
    };
    match FileType::from_extension(ext) {
        Some(FileType::TypeScript | FileType::Vue | FileType::Svelte) => "js",
        Some(FileType::Kotlin | FileType::Java | FileType::Scala | FileType::Groovy) => "jvm",
        Some(FileType::Swift | FileType::ObjC | FileType::Cpp) => "c",
        Some(FileType::Css | FileType::Scss | FileType::Less) => "css",
        Some(FileType::Ruby) => "ruby",
        Some(FileType::Python) => "python",
        Some(FileType::Go) => "go",
        Some(FileType::Rust) => "rust",
        Some(FileType::CSharp) => "csharp",
        Some(FileType::Dart) => "dart",
        Some(FileType::Php) => "php",
        Some(FileType::Perl) => "perl",
        Some(FileType::Lua) => "lua",
        Some(FileType::Elixir) => "elixir",
        Some(FileType::Zig) => "zig",
        Some(_) => "misc",
        None => "other",
    }
}

/// The bare name a reference would use for a definition.
///
/// Parsers store Ruby classes fully qualified (`Billing::Invoice`), singleton
/// methods as `self.call`, attribute readers as `:name`; references only ever
/// carry the last segment. Names with whitespace are DSL blocks (`it "..."`,
/// `scope :active`) that no reference can name.
pub fn short_name(name: &str) -> Option<&str> {
    if let Some(helper) = rspec_helper_name(name) {
        return Some(helper);
    }
    if name.chars().any(char::is_whitespace) {
        return None;
    }
    let name = name.trim_start_matches(':');
    let name = name.rsplit("::").next().unwrap_or(name);
    let name = name.strip_prefix("self.").unwrap_or(name);
    let name = name.rsplit('.').next().unwrap_or(name);
    (!name.is_empty()).then_some(name)
}

/// `let(:user)`, `let!(:user)` and `subject(:user)` define a helper method
/// `user` for the examples of their block.
fn rspec_helper_name(name: &str) -> Option<&str> {
    let args = ["let(:", "let!(:", "subject(:"]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))?;
    let helper = args.strip_suffix(')')?;
    let mut chars = helper.chars();
    let valid = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '?' || c == '!');
    valid.then_some(helper)
}

/// `qual` equals `rel` or ends with `::rel`.
fn is_path_suffix(qual: &str, rel: &str) -> bool {
    qual == rel
        || (qual.len() > rel.len() + 2
            && qual.ends_with(rel)
            && qual[..qual.len() - rel.len()].ends_with("::"))
}

// ---------------------------------------------------------------------------
// Query plumbing
// ---------------------------------------------------------------------------

/// Filters narrowing which definitions a user-supplied symbol name selects.
#[derive(Clone, Debug, Default)]
pub struct SymbolFilter {
    pub in_file: Option<String>,
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphState {
    pub built: bool,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_at: Option<i64>,
}

impl From<&db::SymbolGraphState> for GraphState {
    fn from(state: &db::SymbolGraphState) -> Self {
        GraphState {
            built: state.built,
            stale: state.stale,
            built_at: state.built_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SymbolRef {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: i64,
}

impl SymbolRef {
    fn from_info(info: &GraphSymbolInfo, resolver: &PathResolver) -> SymbolRef {
        SymbolRef {
            name: info.name.clone(),
            kind: info.kind.clone(),
            path: resolver.resolve_with_root(&info.path, info.root_path.as_deref()),
            line: info.line,
        }
    }

    fn render(&self) -> String {
        format!(
            "{} [{}] {}:{}",
            self.name.cyan(),
            self.kind,
            self.path,
            self.line
        )
    }
}

const NOT_BUILT: &str = "Symbol graph not built. Run 'ast-index graph build' first.";
const STALE: &str = "Symbol graph is stale: the index changed since 'graph build'. \
                     Results may be outdated; rerun 'ast-index graph build' or pass --refresh.";

/// Open the index and check the graph. `Ok(None)` means the caller has
/// nothing to query and a notice has already been printed.
fn open_graph(
    root: &Path,
    refresh: bool,
    format: &str,
) -> Result<Option<(db::LeasedConnection, db::SymbolGraphState)>> {
    if !db::db_exists(root) {
        if format == "json" {
            println!(
                "{}",
                serde_json::json!({
                    "error": "index not found; run 'ast-index rebuild' first",
                    "graph": { "built": false, "stale": false },
                })
            );
        } else {
            println!(
                "{}",
                "Index not found. Run 'ast-index rebuild' first.".red()
            );
        }
        return Ok(None);
    }
    let mut conn = db::open_db_leased(root)?;
    let mut state = db::symbol_graph_state(&conn)?;
    if refresh && (!state.built || state.stale) {
        build_symbol_graph(&mut conn, root, false)?;
        state = db::symbol_graph_state(&conn)?;
    }
    if !state.built {
        if format == "json" {
            println!(
                "{}",
                serde_json::json!({
                    "error": "symbol graph not built; run 'ast-index graph build'",
                    "graph": GraphState::from(&state),
                })
            );
        } else {
            println!("{}", NOT_BUILT.yellow());
        }
        return Ok(None);
    }
    if state.stale && format != "json" {
        println!("{}", STALE.yellow());
    }
    Ok(Some((conn, state)))
}

/// Definitions a symbol spec denotes: `Name`, a qualified `Outer::Name`, or
/// `Container#member` to pick a member of one class. A container matches by
/// namespace suffix (`Invoice#total` also finds `Billing::Invoice#total`)
/// unless it is written absolute (`::Invoice#total`).
pub fn resolve_symbol_spec(
    conn: &Connection,
    spec: &str,
    filter: &SymbolFilter,
) -> Result<Vec<GraphSymbolInfo>> {
    let (container, member) = match spec.split_once('#') {
        Some((container, member)) if !container.is_empty() && !member.is_empty() => {
            (Some(container), member)
        }
        _ => (None, spec),
    };
    let lookup = short_name(member).unwrap_or(member);
    let mut found: Vec<GraphSymbolInfo> = db::find_graph_symbols_by_name(conn, lookup)?
        .into_iter()
        .filter(|info| {
            is_node_kind(&info.kind)
                && !db::is_third_party_path(&info.path)
                && filter
                    .in_file
                    .as_deref()
                    .map(|needle| info.path.contains(needle))
                    .unwrap_or(true)
                && filter
                    .kind
                    .as_deref()
                    .map(|kind| info.kind == kind)
                    .unwrap_or(true)
        })
        .collect();

    if let Some(container) = container {
        let absolute = container.starts_with("::");
        let container = container.trim_start_matches("::");
        let mut kept = Vec::new();
        for info in found {
            if let Some(owner) = db::find_enclosing_container(conn, info.id)? {
                let owner_name = owner.name.trim_start_matches("::");
                if owner_name == container || (!absolute && is_path_suffix(owner_name, container)) {
                    kept.push(info);
                }
            }
        }
        return Ok(kept);
    }

    let wanted = spec.trim_start_matches("::");
    if found
        .iter()
        .any(|info| info.name.trim_start_matches("::") == wanted)
    {
        found.retain(|info| info.name.trim_start_matches("::") == wanted);
    } else if spec.contains("::") {
        found.retain(|info| is_path_suffix(info.name.trim_start_matches("::"), wanted));
    }
    Ok(found)
}

/// The matched symbols plus, for class-like ones, every definition inside
/// them: a class's own edges are only its superclass and mixins, while the
/// calls its methods make live on the method symbols.
fn with_members(conn: &Connection, matched: &[GraphSymbolInfo]) -> Result<Vec<GraphSymbolInfo>> {
    let mut seen: HashSet<i64> = matched.iter().map(|info| info.id).collect();
    let mut all = matched.to_vec();
    for info in matched {
        if !is_container_kind(&info.kind) && info.kind != "table" {
            continue;
        }
        for member in db::find_member_symbols(conn, info.id)? {
            if seen.insert(member.id) {
                all.push(member);
            }
        }
    }
    Ok(all)
}

fn describe_matches(spec: &str, matches: &[GraphSymbolInfo], format: &str) -> bool {
    if matches.is_empty() {
        if format == "json" {
            println!(
                "{}",
                serde_json::json!({ "error": format!("no symbol matches '{spec}'") })
            );
        } else {
            println!("{}", format!("No symbol matches '{spec}'.").yellow());
        }
        return false;
    }
    true
}

fn infos_for(conn: &Connection, ids: &HashSet<i64>) -> Result<HashMap<i64, GraphSymbolInfo>> {
    let list: Vec<i64> = ids.iter().copied().collect();
    db::load_graph_symbol_infos(conn, &list)
}

/// Definitions that depend on each of `seeds` through a resolved edge —
/// callers, subclasses, readers — at most `limit` per seed, from the symbol
/// graph. `None` when the graph is not built or the index changed since, so
/// the caller can fall back to matching references by name.
pub fn resolved_dependents_of(
    conn: &Connection,
    seeds: &[db::SearchResult],
    limit: usize,
) -> Result<Option<Vec<Vec<db::SearchResult>>>> {
    let state = db::symbol_graph_state(conn)?;
    if !state.built || state.stale {
        return Ok(None);
    }
    let ids: Vec<Option<i64>> = seeds
        .iter()
        .map(|seed| {
            db::find_symbol_id(
                conn,
                seed.root_path.as_deref(),
                &seed.path,
                seed.line,
                &seed.name,
            )
        })
        .collect::<Result<_>>()?;
    let known: Vec<i64> = ids.iter().flatten().copied().collect();
    let edges = db::load_symbol_edges_to(conn, &known, Confidence::Unique.code())?;
    let mut by_target: HashMap<i64, Vec<i64>> = HashMap::new();
    for edge in &edges {
        if edge.source_id != edge.target_id {
            by_target
                .entry(edge.target_id)
                .or_default()
                .push(edge.source_id);
        }
    }
    let needed: HashSet<i64> = edges.iter().map(|edge| edge.source_id).collect();
    let infos = infos_for(conn, &needed)?;
    let result = ids
        .iter()
        .map(|id| {
            let sources = id
                .and_then(|id| by_target.get(&id))
                .map_or(&[][..], Vec::as_slice);
            sources
                .iter()
                .filter_map(|source| infos.get(source))
                .take(limit)
                .map(|info| db::SearchResult {
                    name: info.name.clone(),
                    qualified_name: None,
                    kind: info.kind.clone(),
                    line: info.line,
                    signature: None,
                    path: info.path.clone(),
                    root_path: info.root_path.clone(),
                })
                .collect()
        })
        .collect();
    Ok(Some(result))
}

/// Print the definitions `spec` matched, at most `limit` of them, after a
/// warning when there are several: their edges are answered as one.
fn print_matched(spec: &str, matched: &[SymbolRef], limit: usize) {
    let shown = limit.max(1);
    if matched.len() > 1 {
        let name = short_name(spec.rsplit('#').next().unwrap_or(spec)).unwrap_or(spec);
        println!(
            "  {}",
            format!(
                "{} definitions match '{spec}' and their edges are merged; narrow with \
                 'Outer::{name}' or 'Class#{name}', --in-file or --kind.",
                matched.len()
            )
            .yellow()
        );
    }
    for subject in matched.iter().take(shown) {
        println!("  {}", subject.render());
    }
    if matched.len() > shown {
        println!(
            "  … and {} more definition(s) (--limit lists more).",
            matched.len() - shown
        );
    }
}

/// What a query about a schema column has to say: its edges come from reads
/// inside the model only, so few or none of the reads in the code show up.
fn column_note(matched: &[GraphSymbolInfo]) -> Option<String> {
    let column = matched.iter().find(|info| info.kind == "column")?;
    let attribute = column
        .name
        .split_once('.')
        .map_or(column.name.as_str(), |(_, attribute)| attribute);
    Some(format!(
        "Column edges come only from reads inside the model of its table \
         ('{attribute}', 'self.{attribute}', '{attribute}?'); a read on another receiver \
         ('record.{attribute}') is never resolved to the column. 'ast-index usages \
         {attribute}' lists every read."
    ))
}

// ---------------------------------------------------------------------------
// graph build / status
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatusReport {
    graph: GraphState,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<serde_json::Value>,
}

pub fn cmd_graph_build(root: &Path, verbose: bool, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }
    let mut conn = db::open_db_leased(root)?;
    let summary = build_symbol_graph(&mut conn, root, verbose)?;
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }
    render_summary(&summary);
    Ok(())
}

pub fn cmd_graph_status(root: &Path, format: &str) -> Result<()> {
    if !db::db_exists(root) {
        println!(
            "{}",
            "Index not found. Run 'ast-index rebuild' first.".red()
        );
        return Ok(());
    }
    let conn = db::open_db_leased(root)?;
    let state = db::symbol_graph_state(&conn)?;
    let summary: Option<GraphBuildSummary> = state
        .summary
        .as_deref()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|value| summary_from_json(&value));
    if format == "json" {
        let report = StatusReport {
            graph: GraphState::from(&state),
            summary: state
                .summary
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if !state.built {
        println!("{}", NOT_BUILT.yellow());
        return Ok(());
    }
    if state.stale {
        println!("{}", STALE.yellow());
    } else {
        println!("{}", "Symbol graph is up to date.".green());
    }
    if let Some(summary) = summary {
        render_summary(&summary);
    }
    Ok(())
}

fn summary_from_json(value: &serde_json::Value) -> Option<GraphBuildSummary> {
    let number = |key: &str| value.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    Some(GraphBuildSummary {
        nodes: number("nodes"),
        edges: number("edges"),
        resolved_edges: number("resolved_edges"),
        references_seen: number("references_seen"),
        references_linked: number("references_linked"),
        by_confidence: value
            .get("by_confidence")?
            .as_array()?
            .iter()
            .map(|entry| ConfidenceCount {
                confidence: entry
                    .get("confidence")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                edges: entry.get("edges").and_then(|v| v.as_u64()).unwrap_or(0),
                references: entry
                    .get("references")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            })
            .collect(),
        dropped: value
            .get("dropped")?
            .as_array()?
            .iter()
            .map(|entry| DropCount {
                reason: entry
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                references: entry
                    .get("references")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            })
            .collect(),
        ambiguity_cap: number("ambiguity_cap") as usize,
        dependents_depth: number("dependents_depth") as usize,
        elapsed_ms: u128::from(number("elapsed_ms")),
        schema: value
            .get("schema")
            .and_then(|schema| serde_json::from_value(schema.clone()).ok()),
    })
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        100.0 * part as f64 / whole as f64
    }
}

fn render_summary(summary: &GraphBuildSummary) {
    println!(
        "{}",
        format!(
            "Symbol graph: {} nodes, {} edges ({} resolved), built in {}ms.",
            summary.nodes, summary.edges, summary.resolved_edges, summary.elapsed_ms
        )
        .bold()
    );
    println!("  Edges by target confidence:");
    for level in &summary.by_confidence {
        println!(
            "    {:<10} {:>9} edges ({:>5.1}%)  {:>9} refs",
            level.confidence,
            level.edges,
            percent(level.edges, summary.edges),
            level.references
        );
    }
    println!(
        "  References: {} seen, {} linked ({:.1}%).",
        summary.references_seen,
        summary.references_linked,
        percent(summary.references_linked, summary.references_seen)
    );
    if !summary.dropped.is_empty() {
        println!("  Not linked:");
        for drop in &summary.dropped {
            println!(
                "    {:<22} {:>9} ({:>5.1}%)",
                drop.reason,
                drop.references,
                percent(drop.references, summary.references_seen)
            );
        }
    }
    if let Some(schema) = &summary.schema {
        render_schema(schema);
    }
    println!(
        "  {}",
        format!(
            "Metrics count resolved edges only; ambiguous edges (up to {} candidates) are listed with --include-ambiguous.",
            summary.ambiguity_cap
        )
        .dimmed()
    );
}

fn render_schema(schema: &SchemaSummary) {
    let link = &schema.link;
    let rules: Vec<String> = link
        .by_rule
        .iter()
        .map(|(rule, models)| {
            let name = serde_json::to_value(rule)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            format!("{name} {models}")
        })
        .collect();
    println!(
        "  Schema: {} tables, {} columns; {} tables read by {} models ({}).",
        link.tables,
        link.columns,
        link.tables_linked,
        link.models_linked,
        rules.join(", ")
    );
    println!(
        "    Unmatched: {} tables without a model, {} models without a table, {} models with two candidate tables (--format json lists them).",
        link.tables_without_model.len(),
        link.models_without_table.len(),
        link.ambiguous_models.len()
    );
    println!(
        "    Column edges: {} ({} references).",
        schema.column_edges, schema.column_references
    );
}

// ---------------------------------------------------------------------------
// graph dependents / dependencies
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Who depends on the symbol (incoming edges).
    Dependents,
    /// What the symbol depends on (outgoing edges).
    Dependencies,
}

#[derive(Debug, Serialize)]
struct EdgeItem {
    /// The symbol the query was about.
    subject: SymbolRef,
    /// The symbol on the other end of the edge.
    other: SymbolRef,
    confidence: Confidence,
    /// Definitions sharing the referenced name (1 when resolved).
    candidates: u32,
    references: u32,
    /// First reference line inside the source symbol.
    line: i64,
}

#[derive(Debug, Serialize)]
struct EdgeReport {
    graph: GraphState,
    direction: &'static str,
    matched: Vec<SymbolRef>,
    resolved_edges: usize,
    ambiguous_edges: usize,
    include_ambiguous: bool,
    /// Whether definitions inside matched classes were included.
    members: bool,
    /// Whether dependents defined in test files were left out; they are not
    /// in `resolved_edges` / `ambiguous_edges` then.
    exclude_tests: bool,
    /// Edges left out by `exclude_tests`.
    excluded_test_edges: usize,
    /// What the answer leaves out, in words.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
    #[serde(flatten)]
    page: Page<EdgeItem>,
}

/// Edges whose other end — the dependent — is defined in a test file
/// ([`is_test_path`]), left out of `edges`. Returns how many were.
fn drop_test_dependents(conn: &Connection, edges: &mut Vec<SymbolEdgeRow>) -> Result<usize> {
    let sources: HashSet<i64> = edges.iter().map(|edge| edge.source_id).collect();
    let infos = infos_for(conn, &sources)?;
    let before = edges.len();
    edges.retain(|edge| {
        infos
            .get(&edge.source_id)
            .is_none_or(|info| !is_test_path(&info.path))
    });
    Ok(before - edges.len())
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_graph_edges(
    root: &Path,
    spec: &str,
    direction: Direction,
    include_ambiguous: bool,
    members: bool,
    exclude_tests: bool,
    filter: &SymbolFilter,
    limit: usize,
    refresh: bool,
    format: &str,
) -> Result<()> {
    let Some((conn, state)) = open_graph(root, refresh, format)? else {
        return Ok(());
    };
    let matched = resolve_symbol_spec(&conn, spec, filter)?;
    if !describe_matches(spec, &matched, format) {
        return Ok(());
    }
    let resolver = PathResolver::from_conn(root, &conn).with_decoration(format != "json");
    let subjects = if members {
        with_members(&conn, &matched)?
    } else {
        matched.clone()
    };
    let subject_ids: HashSet<i64> = subjects.iter().map(|info| info.id).collect();
    let ids: Vec<i64> = subject_ids.iter().copied().collect();
    let all_edges = match direction {
        Direction::Dependents => {
            db::load_symbol_edges_to(&conn, &ids, Confidence::Ambiguous.code())?
        }
        Direction::Dependencies => {
            db::load_symbol_edges_from(&conn, &ids, Confidence::Ambiguous.code())?
        }
    };
    // Edges between two members of the same class are internal to it.
    let mut all_edges: Vec<SymbolEdgeRow> = all_edges
        .into_iter()
        .filter(|edge| {
            !(subject_ids.contains(&edge.source_id) && subject_ids.contains(&edge.target_id))
        })
        .collect();
    let exclude_tests = exclude_tests && direction == Direction::Dependents;
    let excluded_test_edges = if exclude_tests {
        drop_test_dependents(&conn, &mut all_edges)?
    } else {
        0
    };
    let ambiguous_edges = all_edges
        .iter()
        .filter(|edge| !Confidence::from_code(edge.confidence).is_resolved())
        .count();
    let resolved_edges = all_edges.len() - ambiguous_edges;
    let edges: Vec<SymbolEdgeRow> = all_edges
        .into_iter()
        .filter(|edge| include_ambiguous || Confidence::from_code(edge.confidence).is_resolved())
        .collect();

    let mut needed: HashSet<i64> = HashSet::new();
    for edge in &edges {
        needed.insert(edge.source_id);
        needed.insert(edge.target_id);
    }
    let infos = infos_for(&conn, &needed)?;
    let mut items: Vec<EdgeItem> = edges
        .iter()
        .filter_map(|edge| {
            let (subject_id, other_id) = match direction {
                Direction::Dependents => (edge.target_id, edge.source_id),
                Direction::Dependencies => (edge.source_id, edge.target_id),
            };
            let subject = infos.get(&subject_id)?;
            let other = infos.get(&other_id)?;
            if !resolver.matches_filter(other.root_path.as_deref()) {
                return None;
            }
            Some(EdgeItem {
                subject: SymbolRef::from_info(subject, &resolver),
                other: SymbolRef::from_info(other, &resolver),
                confidence: Confidence::from_code(edge.confidence),
                candidates: edge.candidates,
                references: edge.ref_count,
                line: edge.line,
            })
        })
        .collect();
    items.sort_by(|a, b| {
        a.confidence
            .cmp(&b.confidence)
            .then_with(|| a.other.path.cmp(&b.other.path))
            .then_with(|| a.other.line.cmp(&b.other.line))
    });
    let total = items.len();
    let mut notes = Vec::new();
    if direction == Direction::Dependents {
        notes.extend(column_note(&matched));
    }
    if exclude_tests {
        notes.push(format!(
            "{excluded_test_edges} edge(s) from test files left out (--exclude-tests)."
        ));
    }
    let report = EdgeReport {
        graph: GraphState::from(&state),
        direction: match direction {
            Direction::Dependents => "dependents",
            Direction::Dependencies => "dependencies",
        },
        matched: matched
            .iter()
            .map(|info| SymbolRef::from_info(info, &resolver))
            .collect(),
        resolved_edges,
        ambiguous_edges,
        include_ambiguous,
        members,
        exclude_tests,
        excluded_test_edges,
        notes,
        page: Page::new(items, total, limit),
    };
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let (title, arrow) = match direction {
        Direction::Dependents => ("Dependents of", "<-"),
        Direction::Dependencies => ("Dependencies of", "->"),
    };
    println!("{}", format!("{title} '{spec}':").bold());
    print_matched(spec, &report.matched, limit);
    println!(
        "  {} resolved edge(s), {} ambiguous{}.",
        report.resolved_edges,
        report.ambiguous_edges,
        if report.ambiguous_edges > 0 && !include_ambiguous {
            " (hidden; --include-ambiguous lists them)"
        } else {
            ""
        }
    );
    for note in &report.notes {
        println!("  {}", note.yellow());
    }
    let multi = report.matched.len() > 1 || members;
    for item in &report.page.items {
        let level = if item.confidence.is_resolved() {
            format!("[{}]", item.confidence.as_str())
                .green()
                .to_string()
        } else {
            format!("[ambiguous 1/{}]", item.candidates)
                .yellow()
                .to_string()
        };
        let via = if multi {
            format!(" {arrow} {}", item.subject.name)
        } else {
            String::new()
        };
        println!(
            "  {} {} [{}] {}:{} ({} ref{}){}",
            level,
            item.other.name.cyan(),
            item.other.kind,
            item.other.path,
            match direction {
                Direction::Dependents => item.line,
                Direction::Dependencies => item.other.line,
            },
            item.references,
            if item.references == 1 { "" } else { "s" },
            via
        );
    }
    if report.page.items.is_empty() {
        println!("  No edges.");
    }
    super::print_truncation_notice(report.page.pagination);
    Ok(())
}

// ---------------------------------------------------------------------------
// graph impact
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ImpactLevel {
    depth: usize,
    symbols: usize,
    files: usize,
}

#[derive(Debug, Serialize)]
struct ImpactItem {
    depth: usize,
    symbol: SymbolRef,
    /// The symbol one step closer to the seed that this one depends on.
    via: String,
    confidence: Confidence,
}

#[derive(Debug, Serialize)]
struct ImpactReport {
    graph: GraphState,
    matched: Vec<SymbolRef>,
    depth: usize,
    include_ambiguous: bool,
    members: bool,
    /// Whether dependents defined in test files were neither counted nor
    /// followed.
    exclude_tests: bool,
    /// Distinct test-file dependents met and left out by `exclude_tests`.
    excluded_test_symbols: usize,
    /// What the answer leaves out, in words.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
    levels: Vec<ImpactLevel>,
    total_symbols: usize,
    total_files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_only_symbols: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_only_files: Option<usize>,
    #[serde(flatten)]
    page: Page<ImpactItem>,
}

struct Reach {
    /// symbol id -> (depth, via symbol id, confidence of the hop)
    visited: HashMap<i64, (usize, i64, u8)>,
    /// Dependents left out because they are defined in test files.
    excluded: HashSet<i64>,
}

fn reverse_reach(
    conn: &Connection,
    seeds: &[i64],
    depth: usize,
    max_code: u8,
    exclude_tests: bool,
) -> Result<Reach> {
    let seed_set: HashSet<i64> = seeds.iter().copied().collect();
    let mut visited: HashMap<i64, (usize, i64, u8)> = HashMap::new();
    let mut excluded: HashSet<i64> = HashSet::new();
    let mut frontier: Vec<i64> = seeds.to_vec();
    for level in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        let mut edges = db::load_symbol_edges_to(conn, &frontier, max_code)?;
        edges.retain(|edge| {
            !seed_set.contains(&edge.source_id) && !visited.contains_key(&edge.source_id)
        });
        if exclude_tests {
            let before: HashSet<i64> = edges.iter().map(|edge| edge.source_id).collect();
            drop_test_dependents(conn, &mut edges)?;
            let kept: HashSet<i64> = edges.iter().map(|edge| edge.source_id).collect();
            excluded.extend(before.difference(&kept));
        }
        edges.sort_by_key(|edge| (edge.confidence, edge.source_id, edge.target_id));
        let mut next = Vec::new();
        for edge in edges {
            if visited.contains_key(&edge.source_id) {
                continue;
            }
            visited.insert(edge.source_id, (level, edge.target_id, edge.confidence));
            next.push(edge.source_id);
        }
        frontier = next;
    }
    Ok(Reach { visited, excluded })
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_graph_impact(
    root: &Path,
    spec: &str,
    depth: usize,
    include_ambiguous: bool,
    members: bool,
    exclude_tests: bool,
    filter: &SymbolFilter,
    limit: usize,
    refresh: bool,
    format: &str,
) -> Result<()> {
    let Some((conn, state)) = open_graph(root, refresh, format)? else {
        return Ok(());
    };
    let matched = resolve_symbol_spec(&conn, spec, filter)?;
    if !describe_matches(spec, &matched, format) {
        return Ok(());
    }
    let resolver = PathResolver::from_conn(root, &conn).with_decoration(format != "json");
    let seed_infos = if members {
        with_members(&conn, &matched)?
    } else {
        matched.clone()
    };
    let seeds: Vec<i64> = seed_infos.iter().map(|info| info.id).collect();
    let depth = depth.max(1);
    let reach = reverse_reach(
        &conn,
        &seeds,
        depth,
        max_confidence(include_ambiguous),
        exclude_tests,
    )?;
    let resolved_only = if include_ambiguous {
        Some(reverse_reach(
            &conn,
            &seeds,
            depth,
            max_confidence(false),
            exclude_tests,
        )?)
    } else {
        None
    };

    let mut needed: HashSet<i64> = reach.visited.keys().copied().collect();
    needed.extend(reach.visited.values().map(|(_, via, _)| *via));
    if let Some(resolved) = &resolved_only {
        needed.extend(resolved.visited.keys().copied());
    }
    let infos = infos_for(&conn, &needed)?;
    let file_key = |id: &i64| -> Option<(Option<String>, String)> {
        infos
            .get(id)
            .map(|info| (info.root_path.clone(), info.path.clone()))
    };

    let mut levels = Vec::new();
    let mut all_files: HashSet<(Option<String>, String)> = HashSet::new();
    for level in 1..=depth {
        let ids: Vec<&i64> = reach
            .visited
            .iter()
            .filter(|(_, (d, _, _))| *d == level)
            .map(|(id, _)| id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let files: HashSet<(Option<String>, String)> =
            ids.iter().filter_map(|id| file_key(id)).collect();
        all_files.extend(files.iter().cloned());
        levels.push(ImpactLevel {
            depth: level,
            symbols: ids.len(),
            files: files.len(),
        });
    }
    let (resolved_only_symbols, resolved_only_files) = match &resolved_only {
        Some(resolved) => {
            let files: HashSet<(Option<String>, String)> =
                resolved.visited.keys().filter_map(file_key).collect();
            (Some(resolved.visited.len()), Some(files.len()))
        }
        None => (None, None),
    };

    let mut items: Vec<ImpactItem> = reach
        .visited
        .iter()
        .filter_map(|(id, (level, via, code))| {
            let info = infos.get(id)?;
            if !resolver.matches_filter(info.root_path.as_deref()) {
                return None;
            }
            Some(ImpactItem {
                depth: *level,
                symbol: SymbolRef::from_info(info, &resolver),
                via: infos
                    .get(via)
                    .map(|v| v.name.clone())
                    .or_else(|| {
                        seed_infos
                            .iter()
                            .find(|m| m.id == *via)
                            .map(|m| m.name.clone())
                    })
                    .unwrap_or_default(),
                confidence: Confidence::from_code(*code),
            })
        })
        .collect();
    items.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| a.symbol.path.cmp(&b.symbol.path))
            .then_with(|| a.symbol.line.cmp(&b.symbol.line))
    });
    let total = items.len();
    let mut notes: Vec<String> = column_note(&matched).into_iter().collect();
    if exclude_tests {
        notes.push(format!(
            "{} dependent(s) in test files left out and not followed (--exclude-tests).",
            reach.excluded.len()
        ));
    }
    let report = ImpactReport {
        graph: GraphState::from(&state),
        matched: matched
            .iter()
            .map(|info| SymbolRef::from_info(info, &resolver))
            .collect(),
        depth,
        include_ambiguous,
        members,
        exclude_tests,
        excluded_test_symbols: reach.excluded.len(),
        notes,
        levels,
        total_symbols: reach.visited.len(),
        total_files: all_files.len(),
        resolved_only_symbols,
        resolved_only_files,
        page: Page::new(items, total, limit),
    };
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "Impact of '{spec}' (transitive dependents, depth {depth}, {} edges):",
            if include_ambiguous {
                "resolved + ambiguous"
            } else {
                "resolved"
            }
        )
        .bold()
    );
    print_matched(spec, &report.matched, limit);
    for level in &report.levels {
        println!(
            "  depth {}: {} symbol(s) in {} file(s)",
            level.depth, level.symbols, level.files
        );
    }
    println!(
        "  total: {} symbol(s) in {} file(s)",
        report.total_symbols, report.total_files
    );
    for note in &report.notes {
        println!("  {}", note.yellow());
    }
    if let (Some(symbols), Some(files)) = (report.resolved_only_symbols, report.resolved_only_files)
    {
        println!(
            "  {}",
            format!("resolved edges only: {symbols} symbol(s) in {files} file(s); the rest is an upper bound through ambiguous names")
                .dimmed()
        );
    }
    for item in &report.page.items {
        let level = if item.confidence.is_resolved() {
            item.confidence.as_str().green().to_string()
        } else {
            item.confidence.as_str().yellow().to_string()
        };
        println!(
            "  {} {} [{}] {}:{} -> {} ({})",
            format!("d{}", item.depth).dimmed(),
            item.symbol.name.cyan(),
            item.symbol.kind,
            item.symbol.path,
            item.symbol.line,
            item.via,
            level
        );
    }
    super::print_truncation_notice(report.page.pagination);
    Ok(())
}

// ---------------------------------------------------------------------------
// graph path
// ---------------------------------------------------------------------------

/// Pseudo edge code for stepping from a class into a definition inside it.
const CONTAINS: u8 = u8::MAX;

#[derive(Debug, Serialize)]
struct PathHop {
    symbol: SymbolRef,
    /// How this hop reaches the next one: a confidence level for a
    /// dependency edge, or `contains` for stepping from a class into one of
    /// its own definitions. Absent on the last hop.
    #[serde(skip_serializing_if = "Option::is_none")]
    edge: Option<&'static str>,
}

fn edge_label(code: u8) -> &'static str {
    if code == CONTAINS {
        "contains"
    } else {
        Confidence::from_code(code).as_str()
    }
}

#[derive(Debug, Serialize)]
struct PathReport {
    graph: GraphState,
    from: Vec<SymbolRef>,
    to: Vec<SymbolRef>,
    /// `forward` when `from` depends on `to`; `reverse` when only the
    /// opposite direction connects them.
    #[serde(skip_serializing_if = "Option::is_none")]
    direction: Option<&'static str>,
    length: Option<usize>,
    /// Number of distinct shortest paths (may exceed the listed ones).
    shortest_paths: u64,
    include_ambiguous: bool,
    #[serde(flatten)]
    page: Page<Vec<PathHop>>,
}

/// Breadth-first search along dependency edges from `from` to `to`.
///
/// Returns the shortest distance, the per-node predecessor lists restricted
/// to shortest paths, and the reached targets. Only shortest paths are
/// enumerated: all simple paths explode combinatorially on a monorepo graph.
fn shortest_paths(
    conn: &Connection,
    from: &[i64],
    to: &HashSet<i64>,
    max_depth: usize,
    max_code: u8,
) -> Result<Option<ShortestPaths>> {
    let mut distance: HashMap<i64, usize> = from.iter().map(|&id| (id, 0)).collect();
    let mut preds: Predecessors = HashMap::new();
    let mut frontier: Vec<i64> = from.to_vec();
    for level in 1..=max_depth {
        if frontier.is_empty() {
            break;
        }
        // Dispatch into a class (`Service.call` running `process`) is not
        // statically visible, so a class may also step into its members.
        let mut steps: Vec<(i64, i64, u8)> = db::load_symbol_edges_from(conn, &frontier, max_code)?
            .into_iter()
            .map(|edge| (edge.source_id, edge.target_id, edge.confidence))
            .collect();
        steps.extend(
            db::load_member_links(conn, &frontier)?
                .into_iter()
                .map(|(container, member)| (container, member, CONTAINS)),
        );
        let mut next: Vec<i64> = Vec::new();
        for (source, target, code) in steps {
            match distance.get(&target) {
                Some(&d) if d < level => continue,
                Some(_) => {}
                None => {
                    distance.insert(target, level);
                    next.push(target);
                }
            }
            preds.entry(target).or_default().push((source, code));
        }
        let reached: Vec<i64> = next.iter().copied().filter(|id| to.contains(id)).collect();
        if !reached.is_empty() {
            return Ok(Some(ShortestPaths {
                length: level,
                preds,
                reached,
            }));
        }
        frontier = next;
    }
    Ok(None)
}

/// node -> (predecessor on a shortest path, confidence of that edge)
type Predecessors = HashMap<i64, Vec<(i64, u8)>>;

struct ShortestPaths {
    length: usize,
    preds: Predecessors,
    reached: Vec<i64>,
}

fn count_paths(
    node: i64,
    sources: &HashSet<i64>,
    preds: &Predecessors,
    memo: &mut HashMap<i64, u64>,
) -> u64 {
    if sources.contains(&node) {
        return 1;
    }
    if let Some(&count) = memo.get(&node) {
        return count;
    }
    let count = preds
        .get(&node)
        .map(|list| {
            list.iter()
                .map(|(pred, _)| count_paths(*pred, sources, preds, memo))
                .fold(0u64, u64::saturating_add)
        })
        .unwrap_or(0);
    memo.insert(node, count);
    count
}

/// A path as `(symbol id, confidence of the edge to the next hop)`, source
/// first; the last hop carries no confidence.
type PathIds = Vec<(i64, Option<u8>)>;

/// Walk predecessor lists back from `node` to any source, collecting up to
/// `cap` paths. `reversed` holds the hops already chosen, target first.
fn collect_paths(
    node: i64,
    sources: &HashSet<i64>,
    preds: &Predecessors,
    reversed: &mut PathIds,
    out: &mut Vec<PathIds>,
    cap: usize,
) {
    if out.len() >= cap {
        return;
    }
    if sources.contains(&node) {
        let mut path = reversed.clone();
        path.reverse();
        out.push(path);
        return;
    }
    let Some(list) = preds.get(&node) else {
        return;
    };
    for &(pred, code) in list {
        reversed.push((pred, Some(code)));
        collect_paths(pred, sources, preds, reversed, out, cap);
        reversed.pop();
        if out.len() >= cap {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_graph_path(
    root: &Path,
    from: &str,
    to: &str,
    max_depth: usize,
    max_paths: usize,
    include_ambiguous: bool,
    filter_from: &SymbolFilter,
    filter_to: &SymbolFilter,
    refresh: bool,
    format: &str,
) -> Result<()> {
    let Some((conn, state)) = open_graph(root, refresh, format)? else {
        return Ok(());
    };
    let from_matches = resolve_symbol_spec(&conn, from, filter_from)?;
    if !describe_matches(from, &from_matches, format) {
        return Ok(());
    }
    let to_matches = resolve_symbol_spec(&conn, to, filter_to)?;
    if !describe_matches(to, &to_matches, format) {
        return Ok(());
    }
    let resolver = PathResolver::from_conn(root, &conn).with_decoration(format != "json");
    let from_ids: Vec<i64> = with_members(&conn, &from_matches)?
        .iter()
        .map(|info| info.id)
        .collect();
    let to_ids: HashSet<i64> = with_members(&conn, &to_matches)?
        .iter()
        .map(|info| info.id)
        .collect();
    let max_code = max_confidence(include_ambiguous);

    let mut direction = None;
    let mut found = shortest_paths(&conn, &from_ids, &to_ids, max_depth, max_code)?;
    if found.is_some() {
        direction = Some("forward");
    } else {
        let to_list: Vec<i64> = to_ids.iter().copied().collect();
        let from_set: HashSet<i64> = from_ids.iter().copied().collect();
        found = shortest_paths(&conn, &to_list, &from_set, max_depth, max_code)?;
        if found.is_some() {
            direction = Some("reverse");
        }
    }

    let mut paths: Vec<PathIds> = Vec::new();
    let mut length = None;
    let mut shortest = 0u64;
    if let Some(found) = &found {
        length = Some(found.length);
        let sources: HashSet<i64> = if direction == Some("forward") {
            from_ids.iter().copied().collect()
        } else {
            to_ids.clone()
        };
        let mut memo = HashMap::new();
        for &target in &found.reached {
            shortest =
                shortest.saturating_add(count_paths(target, &sources, &found.preds, &mut memo));
            let mut reversed = vec![(target, None)];
            collect_paths(
                target,
                &sources,
                &found.preds,
                &mut reversed,
                &mut paths,
                max_paths,
            );
        }
    }

    let mut needed: HashSet<i64> = HashSet::new();
    for path in &paths {
        needed.extend(path.iter().map(|(id, _)| *id));
    }
    let infos = infos_for(&conn, &needed)?;
    let rendered: Vec<Vec<PathHop>> = paths
        .iter()
        .map(|path| {
            path.iter()
                .filter_map(|(id, code)| {
                    infos.get(id).map(|info| PathHop {
                        symbol: SymbolRef::from_info(info, &resolver),
                        edge: code.map(edge_label),
                    })
                })
                .collect()
        })
        .collect();
    let total = rendered.len();
    let report = PathReport {
        graph: GraphState::from(&state),
        from: from_matches
            .iter()
            .map(|info| SymbolRef::from_info(info, &resolver))
            .collect(),
        to: to_matches
            .iter()
            .map(|info| SymbolRef::from_info(info, &resolver))
            .collect(),
        direction,
        length,
        shortest_paths: shortest,
        include_ambiguous,
        page: Page::new(rendered, total, max_paths),
    };
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    match (report.length, report.direction) {
        (Some(length), Some(direction)) => {
            let (a, b) = if direction == "forward" {
                (from, to)
            } else {
                (to, from)
            };
            println!(
                "{}",
                format!(
                    "'{a}' reaches '{b}' in {length} hop(s); {} shortest path(s), showing {}:",
                    report.shortest_paths,
                    report.page.items.len()
                )
                .bold()
            );
            if direction == "reverse" {
                println!(
                    "  {}",
                    format!("No path from '{from}' to '{to}'; this is the reverse direction.")
                        .yellow()
                );
            }
        }
        _ => {
            println!(
                "{}",
                format!(
                    "No dependency path between '{from}' and '{to}' within {max_depth} hop(s){}.",
                    if include_ambiguous {
                        ""
                    } else {
                        " over resolved edges (try --include-ambiguous)"
                    }
                )
                .yellow()
            );
            return Ok(());
        }
    }
    for (index, path) in report.page.items.iter().enumerate() {
        println!("  path {}:", index + 1);
        for hop in path {
            let edge = match hop.edge {
                Some("ambiguous") => " -> [ambiguous]".yellow().to_string(),
                Some("contains") => " -> [contains]".dimmed().to_string(),
                Some(label) => format!(" -> [{label}]").green().to_string(),
                None => String::new(),
            };
            println!("    {}{}", hop.symbol.render(), edge);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// graph cycles
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CycleItem {
    size: usize,
    files: usize,
    /// One concrete cycle through the component, first symbol repeated last.
    example: Vec<SymbolRef>,
    members: Vec<SymbolRef>,
    members_truncated: bool,
}

#[derive(Debug, Serialize)]
struct CycleReport {
    graph: GraphState,
    components: usize,
    #[serde(flatten)]
    page: Page<CycleItem>,
}

/// Strongly connected components with at least two members (iterative
/// Tarjan, so deep dependency chains cannot overflow the stack).
fn strongly_connected(adjacency: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adjacency.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = Vec::new();
    let mut counter = 0usize;
    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        let mut call: Vec<(usize, usize)> = vec![(start, 0)];
        index[start] = counter;
        low[start] = counter;
        counter += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&mut (node, ref mut next_child)) = call.last_mut() {
            if *next_child < adjacency[node].len() {
                let child = adjacency[node][*next_child];
                *next_child += 1;
                if index[child] == usize::MAX {
                    index[child] = counter;
                    low[child] = counter;
                    counter += 1;
                    stack.push(child);
                    on_stack[child] = true;
                    call.push((child, 0));
                } else if on_stack[child] {
                    low[node] = low[node].min(index[child]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == index[node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    if component.len() > 1 {
                        components.push(component);
                    }
                }
            }
        }
    }
    components
}

/// Shortest cycle through `start` that stays inside `members`.
fn cycle_through(start: usize, members: &HashSet<usize>, adjacency: &[Vec<usize>]) -> Vec<usize> {
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for &next in &adjacency[node] {
            if !members.contains(&next) {
                continue;
            }
            if next == start {
                let mut path = vec![node];
                let mut current = node;
                while current != start {
                    current = parent[&current];
                    path.push(current);
                }
                path.reverse();
                path.push(start);
                return path;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = parent.entry(next) {
                slot.insert(node);
                queue.push_back(next);
            }
        }
    }
    vec![start]
}

pub fn cmd_graph_cycles(
    root: &Path,
    limit: usize,
    min_size: usize,
    path_prefix: Option<&str>,
    refresh: bool,
    format: &str,
) -> Result<()> {
    let Some((conn, state)) = open_graph(root, refresh, format)? else {
        return Ok(());
    };
    let resolver = PathResolver::from_conn(root, &conn).with_decoration(format != "json");
    let edges = db::load_all_symbol_edges(&conn, max_confidence(false))?;
    let mut dense: HashMap<i64, usize> = HashMap::new();
    let mut ids: Vec<i64> = Vec::new();
    let mut adjacency: Vec<Vec<usize>> = Vec::new();
    for edge in &edges {
        for id in [edge.source_id, edge.target_id] {
            if let std::collections::hash_map::Entry::Vacant(slot) = dense.entry(id) {
                slot.insert(ids.len());
                ids.push(id);
                adjacency.push(Vec::new());
            }
        }
        adjacency[dense[&edge.source_id]].push(dense[&edge.target_id]);
    }
    let mut components: Vec<Vec<usize>> = strongly_connected(&adjacency)
        .into_iter()
        .filter(|component| component.len() >= min_size.max(2))
        .collect();

    let mut needed: HashSet<i64> = HashSet::new();
    for component in &components {
        needed.extend(component.iter().map(|&node| ids[node]));
    }
    let infos = infos_for(&conn, &needed)?;
    if let Some(prefix) = path_prefix {
        components.retain(|component| {
            component.iter().any(|&node| {
                infos
                    .get(&ids[node])
                    .is_some_and(|info| info.path.starts_with(prefix))
            })
        });
    }
    components.retain(|component| {
        component.iter().any(|&node| {
            infos
                .get(&ids[node])
                .is_some_and(|info| resolver.matches_filter(info.root_path.as_deref()))
        })
    });
    for component in components.iter_mut() {
        component.sort_by(|&a, &b| {
            let left = infos.get(&ids[a]);
            let right = infos.get(&ids[b]);
            left.map(|i| (&i.path, i.line))
                .cmp(&right.map(|i| (&i.path, i.line)))
        });
    }
    components.sort_by_key(|component| Reverse(component.len()));
    let total = components.len();

    const MEMBER_LIMIT: usize = 12;
    let items: Vec<CycleItem> = components
        .iter()
        .take(limit)
        .map(|component| {
            let members: HashSet<usize> = component.iter().copied().collect();
            let example = cycle_through(component[0], &members, &adjacency);
            let files: HashSet<&str> = component
                .iter()
                .filter_map(|&node| infos.get(&ids[node]).map(|info| info.path.as_str()))
                .collect();
            let to_ref = |node: &usize| {
                infos
                    .get(&ids[*node])
                    .map(|info| SymbolRef::from_info(info, &resolver))
            };
            CycleItem {
                size: component.len(),
                files: files.len(),
                example: example.iter().filter_map(to_ref).collect(),
                members: component
                    .iter()
                    .take(MEMBER_LIMIT)
                    .filter_map(to_ref)
                    .collect(),
                members_truncated: component.len() > MEMBER_LIMIT,
            }
        })
        .collect();
    let report = CycleReport {
        graph: GraphState::from(&state),
        components: total,
        page: Page::new(items, total, limit),
    };
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{}",
        format!(
            "Dependency cycles over resolved edges: {} component(s) with {}+ symbols.",
            report.components,
            min_size.max(2)
        )
        .bold()
    );
    for item in &report.page.items {
        println!(
            "  {} symbols in {} file(s):",
            item.size.to_string().yellow(),
            item.files
        );
        let chain: Vec<String> = item.example.iter().map(|hop| hop.name.clone()).collect();
        println!("    cycle: {}", chain.join(" -> "));
        for member in &item.members {
            println!("    {}", member.render());
        }
        if item.members_truncated {
            println!(
                "    {}",
                format!("... {} more", item.size - item.members.len()).dimmed()
            );
        }
    }
    if report.page.items.is_empty() {
        println!("  No cycles.");
    }
    super::print_truncation_notice(report.page.pagination);
    Ok(())
}

// ---------------------------------------------------------------------------
// graph top / metrics
// ---------------------------------------------------------------------------

pub const TOP_SORT_KEYS: [&str; 4] = ["pagerank", "fan-in", "fan-out", "dependents"];

#[derive(Debug, Serialize)]
struct MetricsItem {
    symbol: SymbolRef,
    #[serde(flatten)]
    metrics: MetricsView,
}

#[derive(Debug, Serialize)]
struct MetricsView {
    fan_in: u32,
    fan_in_files: u32,
    fan_in_ambiguous: u32,
    fan_out: u32,
    fan_out_ambiguous: u32,
    dependents: u32,
    dependents_depth: usize,
    pagerank: f64,
    pagerank_pct: f64,
}

impl MetricsView {
    fn from(metrics: &SymbolGraphMetrics) -> MetricsView {
        MetricsView {
            fan_in: metrics.fan_in,
            fan_in_files: metrics.fan_in_files,
            fan_in_ambiguous: metrics.fan_in_ambiguous,
            fan_out: metrics.fan_out,
            fan_out_ambiguous: metrics.fan_out_ambiguous,
            dependents: metrics.dependents,
            dependents_depth: DEPENDENTS_DEPTH,
            pagerank: metrics.pagerank,
            pagerank_pct: metrics.pagerank_pct,
        }
    }

    fn render(&self) -> String {
        format!(
            "fan-in {} ({} files, +{} ambiguous) · fan-out {} (+{} ambiguous) · dependents≤{} {} · pagerank {:.2} (p{:.0})",
            self.fan_in,
            self.fan_in_files,
            self.fan_in_ambiguous,
            self.fan_out,
            self.fan_out_ambiguous,
            self.dependents_depth,
            self.dependents,
            self.pagerank,
            self.pagerank_pct
        )
    }
}

#[derive(Debug, Serialize)]
struct MetricsReport {
    graph: GraphState,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<String>,
    #[serde(flatten)]
    page: Page<MetricsItem>,
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_graph_top(
    root: &Path,
    sort: &str,
    limit: usize,
    kind: Option<&str>,
    path_prefix: Option<&str>,
    exclude_tests: bool,
    refresh: bool,
    format: &str,
) -> Result<()> {
    if !TOP_SORT_KEYS.contains(&sort) {
        bail!("--sort must be one of: {}", TOP_SORT_KEYS.join(", "));
    }
    let Some((conn, state)) = open_graph(root, refresh, format)? else {
        return Ok(());
    };
    let resolver = PathResolver::from_conn(root, &conn).with_decoration(format != "json");
    let mut rows = db::load_all_symbol_graph_metrics(&conn)?;
    let key = |m: &SymbolGraphMetrics| -> f64 {
        match sort {
            "fan-in" => f64::from(m.fan_in),
            "fan-out" => f64::from(m.fan_out),
            "dependents" => f64::from(m.dependents),
            _ => m.pagerank,
        }
    };
    rows.sort_by(|a, b| {
        key(b)
            .total_cmp(&key(a))
            .then_with(|| a.symbol_id.cmp(&b.symbol_id))
    });

    // Filters need file paths, so resolve them in growing batches until the
    // page is full instead of joining every metrics row with its symbol.
    let mut items = Vec::new();
    let mut total_matching = 0usize;
    let filtered = kind.is_some() || path_prefix.is_some() || exclude_tests;
    for chunk in rows.chunks(2000) {
        let ids: Vec<i64> = chunk.iter().map(|m| m.symbol_id).collect();
        let infos = db::load_graph_symbol_infos(&conn, &ids)?;
        for metrics in chunk {
            let Some(info) = infos.get(&metrics.symbol_id) else {
                continue;
            };
            if kind.is_some_and(|k| info.kind != k)
                || path_prefix.is_some_and(|p| !info.path.starts_with(p))
                || (exclude_tests && is_test_path(&info.path))
                || !resolver.matches_filter(info.root_path.as_deref())
            {
                continue;
            }
            total_matching += 1;
            if items.len() < limit {
                items.push(MetricsItem {
                    symbol: SymbolRef::from_info(info, &resolver),
                    metrics: MetricsView::from(metrics),
                });
            }
        }
        if items.len() >= limit && !filtered {
            total_matching = rows.len();
            break;
        }
    }
    let report = MetricsReport {
        graph: GraphState::from(&state),
        sort: Some(sort.to_string()),
        page: Page::new(items, total_matching, limit),
    };
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{}",
        format!("Top symbols by {sort} (resolved edges only):").bold()
    );
    for (rank, item) in report.page.items.iter().enumerate() {
        println!("  {:>3}. {}", rank + 1, item.symbol.render());
        println!("       {}", item.metrics.render().dimmed());
    }
    if report.page.items.is_empty() {
        println!("  No symbols matched.");
    }
    super::print_truncation_notice(report.page.pagination);
    Ok(())
}

pub fn cmd_graph_metrics(
    root: &Path,
    specs: &[String],
    filter: &SymbolFilter,
    limit: usize,
    refresh: bool,
    format: &str,
) -> Result<()> {
    let Some((conn, state)) = open_graph(root, refresh, format)? else {
        return Ok(());
    };
    let resolver = PathResolver::from_conn(root, &conn).with_decoration(format != "json");
    let mut matched: Vec<GraphSymbolInfo> = Vec::new();
    for spec in specs {
        matched.extend(resolve_symbol_spec(&conn, spec, filter)?);
    }
    let ids: Vec<i64> = matched.iter().map(|info| info.id).collect();
    let metrics = db::load_symbol_graph_metrics(&conn, &ids)?;
    let mut items: Vec<MetricsItem> = matched
        .iter()
        .map(|info| MetricsItem {
            symbol: SymbolRef::from_info(info, &resolver),
            metrics: MetricsView::from(metrics.get(&info.id).unwrap_or(&SymbolGraphMetrics {
                symbol_id: info.id,
                ..SymbolGraphMetrics::default()
            })),
        })
        .collect();
    items.sort_by(|a, b| b.metrics.pagerank.total_cmp(&a.metrics.pagerank));
    let total = items.len();
    let report = MetricsReport {
        graph: GraphState::from(&state),
        sort: None,
        page: Page::new(items, total, limit),
    };
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if report.page.items.is_empty() {
        println!(
            "{}",
            format!("No symbol matches {}.", specs.join(", ")).yellow()
        );
        return Ok(());
    }
    for item in &report.page.items {
        println!("  {}", item.symbol.render());
        println!("    {}", item.metrics.render());
    }
    super::print_truncation_notice(report.page.pagination);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_strips_qualifiers_and_markers() {
        assert_eq!(short_name("Billing::Invoice"), Some("Invoice"));
        assert_eq!(short_name("self.call"), Some("call"));
        assert_eq!(short_name(":result"), Some("result"));
        assert_eq!(short_name("valid?"), Some("valid?"));
        assert_eq!(short_name("it \"works\""), None);
        assert_eq!(short_name("let(:invoice)"), Some("invoice"));
        assert_eq!(short_name("let!(:paid?)"), Some("paid?"));
        assert_eq!(short_name("subject(:service)"), Some("service"));
    }

    #[test]
    fn tarjan_finds_only_real_cycles() {
        let adjacency = vec![vec![1], vec![2], vec![0], vec![0], vec![4]];
        let components = strongly_connected(&adjacency);
        assert_eq!(components.len(), 1);
        let mut members = components[0].clone();
        members.sort();
        assert_eq!(members, vec![0, 1, 2]);
    }
}
