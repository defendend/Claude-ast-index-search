//! Ranking presets for `search --rank`.
//!
//! A preset re-orders what a query already found; it never adds or drops a
//! match. The candidate pool is the head of the ordinary relevance order
//! (project code only), and relevance keeps its say on top of the score:
//!
//! * **Symbol tiers are hard.** A symbol whose name is exactly what was typed
//!   stays above one whose name merely contains it, which stays above one
//!   that only mentions it in its signature. A preset only re-orders inside a
//!   tier, so `search User --rank central` cannot answer with some unrelated
//!   base class that happens to be more central.
//! * **Position still counts inside a tier.** The final key is a weighted
//!   blend of the preset score and the candidate's relevance position within
//!   its tier, so the tail of the pool needs a clearly better score to
//!   overtake its head.
//! * **Files** all contain the query in their path; where it sits (file stem,
//!   file name, directory) is their relevance term, not a boundary.
//! * **Third-party code** (`db::is_vendor_path`) is never scored and is
//!   listed after every project result.
//!
//! Two kinds of evidence feed the scores, at different granularities:
//!
//! * **File history** (`hotspots --collect`): commits, bugfix ratio, churn,
//!   authors, age. It describes the whole file — every symbol defined in a
//!   file shares it — and is always labelled that way.
//! * **Symbol graph** (`graph build`): resolved fan-in, transitive dependents
//!   and PageRank of the symbol itself. For a file result, the file's
//!   strongest symbol (highest PageRank) stands in for the file.
//!
//! A preset whose evidence is missing is not applied at all, and the output
//! says which command collects it: ranking by zeros would look like a verdict.

use std::collections::HashMap;

use anyhow::{bail, Result};
use colored::Colorize;
use rusqlite::Connection;
use serde::Serialize;

use super::git_signals::{
    self, midrank_percentile, serialize_round3_option, short_sha, FileHistory, HistoryAvailability,
    HistorySnapshot, PERCENTILE_ELEVATED, PERCENTILE_HIGH,
};
use super::graph::DEPENDENTS_DEPTH;
use super::PathResolver;
use crate::db::{self, FileResult, SearchResult, SymbolGraphMetrics};

/// Share of the in-tier sort key that comes from the preset score; the rest
/// comes from the candidate's relevance position in its tier.
///
/// Swept over 11 agent queries on a 40k-file monorepo (weights 0.5-1.0,
/// pools 50-200). With a 100-symbol pool the top five realize 51%, 73%, 93%
/// and 98% of the preset score the tiers allow at weights 0.5, 0.8, 0.9 and
/// 1.0, while the deepest relevance position they reach grows 7, 15, 21, 33.
/// 0.9 is the knee: past it five more points cost a reach half again as deep.
const PRESET_WEIGHT: f64 = 0.9;
/// Relevance position at which the relevance term has fallen to one half:
/// `1 / (1 + position / RELEVANCE_HALF)`. It is the default page size, and
/// fixed rather than tied to `--limit` so a shorter page is always a prefix
/// of a longer one.
const RELEVANCE_HALF: f64 = 20.0;
/// Symbols taken from the head of the relevance order before re-ranking.
const SYMBOL_POOL: usize = 100;
/// Files taken before re-ranking. Path matches carry no relevance order
/// (they come back alphabetically), so the pool is as wide as is cheap.
const FILE_POOL: usize = 2000;
const SECONDS_PER_DAY: i64 = 86_400;

pub const PRESET_NAMES: [&str; 4] = ["proven", "hotspots", "risky", "central"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Proven,
    Hotspots,
    Risky,
    Central,
}

impl Preset {
    pub fn parse(name: &str) -> Result<Preset> {
        Ok(match name {
            "proven" => Preset::Proven,
            "hotspots" => Preset::Hotspots,
            "risky" => Preset::Risky,
            "central" => Preset::Central,
            _ => bail!("--rank must be one of: {}", PRESET_NAMES.join(", ")),
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Proven => "proven",
            Preset::Hotspots => "hotspots",
            Preset::Risky => "risky",
            Preset::Central => "central",
        }
    }

    fn needs_history(self) -> bool {
        self != Preset::Central
    }

    fn needs_graph(self) -> bool {
        self != Preset::Hotspots
    }

    fn formula(self) -> &'static str {
        match self {
            Preset::Proven => {
                "mean of calm (1 - file hotspot score), file age percentile, idle percentile \
                 (days since the file last changed) and used (1 when at least one caller \
                 resolves to the symbol, else 0)"
            }
            Preset::Hotspots => {
                "file hotspot score: mean percentile of commits, churn and bugfix ratio, \
                 the score 'ast-index hotspots' reports"
            }
            Preset::Risky => {
                "blast radius x file hotspot score: percentile of transitive dependents among \
                 referenced symbols (0 when nothing resolves to it) times the hotspot score"
            }
            Preset::Central => {
                "PageRank percentile among referenced symbols (0 when nothing resolves to it)"
            }
        }
    }

    pub fn symbol_pool(limit: usize) -> usize {
        SYMBOL_POOL.max(limit.saturating_add(1))
    }

    pub fn file_pool(limit: usize) -> usize {
        FILE_POOL.max(limit.saturating_add(1))
    }
}

// ---------------------------------------------------------------------------
// Evidence availability
// ---------------------------------------------------------------------------

/// A signal a preset needs but the index does not have yet.
#[derive(Clone, Debug, Serialize)]
pub struct MissingSignal {
    pub signal: &'static str,
    pub reason: &'static str,
    pub command: &'static str,
}

const STALE_GRAPH: &str = "symbol graph is stale: the index changed since 'graph build'; \
                           rerun 'ast-index graph build'";

/// Repository-wide distributions the graph percentiles are measured against:
/// every symbol with at least one resolved caller.
///
/// Most graph nodes have none (69% on a 40k-file monorepo), so against all
/// nodes any caller at all lands above the 69th percentile and two
/// dependents score almost like five hundred. Against referenced symbols
/// only, the percentile spreads over the range that actually varies; a
/// symbol without resolved callers sits below all of them at 0.
struct GraphPopulation {
    fan_in_files: Vec<f64>,
    dependents: Vec<f64>,
    pageranks: Vec<f64>,
}

impl GraphPopulation {
    fn load(conn: &Connection) -> Result<GraphPopulation> {
        let rows: Vec<SymbolGraphMetrics> = db::load_all_symbol_graph_metrics(conn)?
            .into_iter()
            .filter(|metrics| metrics.fan_in > 0)
            .collect();
        let column = |pick: fn(&SymbolGraphMetrics) -> f64| {
            let mut values: Vec<f64> = rows.iter().map(pick).collect();
            values.sort_by(f64::total_cmp);
            values
        };
        Ok(GraphPopulation {
            fan_in_files: column(|m| f64::from(m.fan_in_files)),
            dependents: column(|m| f64::from(m.dependents)),
            pageranks: column(|m| m.pagerank),
        })
    }
}

/// Everything a ranked search needs besides the candidates themselves.
pub struct RankContext {
    preset: Preset,
    history: Option<HistorySnapshot>,
    graph: Option<GraphPopulation>,
    graph_state: Option<db::SymbolGraphState>,
    missing: Vec<MissingSignal>,
    warnings: Vec<&'static str>,
}

impl RankContext {
    pub fn load(conn: &Connection, preset: Preset) -> Result<RankContext> {
        let mut missing = Vec::new();
        let mut warnings = Vec::new();

        let graph_state = if preset.needs_graph() {
            let state = db::symbol_graph_state(conn)?;
            if !state.built {
                missing.push(MissingSignal {
                    signal: "graph",
                    reason: "the symbol graph has not been built",
                    command: "ast-index graph build",
                });
            } else if state.stale {
                warnings.push(STALE_GRAPH);
            }
            Some(state)
        } else {
            None
        };

        let history = if preset.needs_history() {
            match git_signals::load_history_snapshot(conn)? {
                HistoryAvailability::Ready(snapshot) => Some(snapshot),
                HistoryAvailability::NotCollected => {
                    missing.push(MissingSignal {
                        signal: "history",
                        reason: "git history has not been collected",
                        command: "ast-index hotspots --collect",
                    });
                    None
                }
                HistoryAvailability::Empty => {
                    missing.push(MissingSignal {
                        signal: "history",
                        reason: "the collected git history covers no file that still exists",
                        command: "ast-index hotspots --collect --full",
                    });
                    None
                }
            }
        } else {
            None
        };

        let graph = match &graph_state {
            Some(state) if state.built && missing.is_empty() => Some(GraphPopulation::load(conn)?),
            _ => None,
        };

        Ok(RankContext {
            preset,
            history,
            graph,
            graph_state,
            missing,
            warnings,
        })
    }

    pub fn applied(&self) -> bool {
        self.missing.is_empty()
    }

    pub fn preset(&self) -> Preset {
        self.preset
    }
}

// ---------------------------------------------------------------------------
// Dossier: the evidence printed next to every ranked result
// ---------------------------------------------------------------------------

/// File-level history of the file a result lives in.
#[derive(Clone, Debug, Serialize)]
pub struct HistoryDossier {
    pub granularity: &'static str,
    pub hotspot_score: u32,
    pub commits: i64,
    pub commits_pct: u32,
    pub fix_commits: i64,
    pub fix_ratio: f64,
    pub fix_ratio_pct: u32,
    pub churn: i64,
    pub churn_pct: u32,
    pub authors: usize,
    pub authors_pct: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_days: Option<f64>,
    pub age_pct: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_since_change: Option<f64>,
    pub idle_pct: u32,
    pub labels: Vec<String>,
}

impl HistoryDossier {
    fn from(history: &FileHistory) -> HistoryDossier {
        let hotspot = &history.hotspot;
        HistoryDossier {
            granularity: "file",
            hotspot_score: hotspot.score,
            commits: hotspot.commits,
            commits_pct: hotspot.commits_pct,
            fix_commits: hotspot.fix_commits,
            fix_ratio: hotspot.fix_ratio,
            fix_ratio_pct: hotspot.fix_ratio_pct,
            churn: hotspot.churn,
            churn_pct: hotspot.churn_pct,
            authors: hotspot.authors,
            authors_pct: hotspot.authors_pct,
            age_days: hotspot.age_days,
            age_pct: hotspot.age_pct,
            days_since_change: hotspot.days_since_change,
            idle_pct: history.idle_pct,
            labels: hotspot.labels.clone(),
        }
    }

    fn render(&self) -> String {
        let mut parts = vec![
            format!("{} commits (p{})", self.commits, self.commits_pct),
            format!(
                "fixes {}/{} = {:.0}% (p{})",
                self.fix_commits,
                self.commits,
                self.fix_ratio * 100.0,
                self.fix_ratio_pct
            ),
            format!("churn {} lines (p{})", self.churn, self.churn_pct),
            format!("{} authors (p{})", self.authors, self.authors_pct),
        ];
        if let Some(age) = self.age_days {
            parts.push(format!("age {age:.0}d (p{})", self.age_pct));
        }
        if let Some(idle) = self.days_since_change {
            parts.push(format!(
                "last change {idle:.0}d ago (idle p{})",
                self.idle_pct
            ));
        }
        let mut line = format!("file history: {}", parts.join(" · "));
        if !self.labels.is_empty() {
            line.push_str(&format!(" · {}", self.labels.join(" ")));
        }
        line
    }
}

/// The symbol a file result borrows its graph evidence from.
#[derive(Clone, Debug, Serialize)]
pub struct StrongestSymbol {
    pub name: String,
    pub kind: String,
    pub line: i64,
}

/// Graph position of a symbol, or of a file's strongest symbol.
#[derive(Clone, Debug, Serialize)]
pub struct GraphDossier {
    pub granularity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strongest_symbol: Option<StrongestSymbol>,
    pub in_graph: bool,
    pub fan_in: u32,
    pub fan_in_files: u32,
    pub fan_in_files_pct: f64,
    pub fan_in_ambiguous: u32,
    pub fan_out: u32,
    pub fan_out_ambiguous: u32,
    pub dependents: u32,
    pub dependents_pct: f64,
    pub dependents_depth: usize,
    pub pagerank: f64,
    pub pagerank_pct: f64,
    pub labels: Vec<String>,
}

impl GraphDossier {
    fn build(
        metrics: Option<&SymbolGraphMetrics>,
        population: &GraphPopulation,
        strongest_symbol: Option<StrongestSymbol>,
    ) -> GraphDossier {
        let in_graph = metrics.is_some();
        let metrics = metrics.copied().unwrap_or_default();
        let referenced = |raw: f64, population: &[f64]| {
            if metrics.fan_in == 0 {
                0.0
            } else {
                round1(midrank_percentile(population, raw))
            }
        };
        let fan_in_files_pct =
            referenced(f64::from(metrics.fan_in_files), &population.fan_in_files);
        let dependents_pct = referenced(f64::from(metrics.dependents), &population.dependents);
        let pagerank_pct = referenced(metrics.pagerank, &population.pageranks);
        let mut labels = Vec::new();
        push_level_label(
            &mut labels,
            "fan-in",
            metrics.fan_in_files,
            fan_in_files_pct,
        );
        push_level_label(
            &mut labels,
            "dependents",
            metrics.dependents,
            dependents_pct,
        );
        if metrics.fan_in > 0 && pagerank_pct >= PERCENTILE_HIGH {
            labels.push("pagerank:high".to_string());
        }
        if metrics.fan_in == 0 && metrics.fan_in_ambiguous > 0 {
            labels.push("callers:unresolved".to_string());
        }
        GraphDossier {
            granularity: if strongest_symbol.is_some() {
                "file"
            } else {
                "symbol"
            },
            strongest_symbol,
            in_graph,
            fan_in: metrics.fan_in,
            fan_in_files: metrics.fan_in_files,
            fan_in_files_pct,
            fan_in_ambiguous: metrics.fan_in_ambiguous,
            fan_out: metrics.fan_out,
            fan_out_ambiguous: metrics.fan_out_ambiguous,
            dependents: metrics.dependents,
            dependents_pct,
            dependents_depth: DEPENDENTS_DEPTH,
            pagerank: metrics.pagerank,
            pagerank_pct,
            labels,
        }
    }

    fn render(&self) -> String {
        let head = match &self.strongest_symbol {
            Some(symbol) => format!(
                "graph via strongest symbol {} [{}]:{}",
                symbol.name, symbol.kind, symbol.line
            ),
            None => "symbol graph".to_string(),
        };
        if !self.in_graph {
            return format!("{head}: no resolved or ambiguous edges");
        }
        let mut line = format!(
            "{head}: fan-in {} from {} files (p{:.0}, +{} ambiguous) · dependents≤{} {} (p{:.0}) · pagerank p{:.1}",
            self.fan_in,
            self.fan_in_files,
            self.fan_in_files_pct,
            self.fan_in_ambiguous,
            self.dependents_depth,
            self.dependents,
            self.dependents_pct,
            self.pagerank_pct,
        );
        if !self.labels.is_empty() {
            line.push_str(&format!(" · {}", self.labels.join(" ")));
        }
        line
    }
}

/// Graph labels use the same percentile thresholds as the history labels,
/// and never fire on a zero: "0 dependents" is not "elevated" anywhere.
fn push_level_label(labels: &mut Vec<String>, name: &str, raw: u32, pct: f64) {
    if raw == 0 {
        return;
    }
    if pct >= PERCENTILE_HIGH {
        labels.push(format!("{name}:high"));
    } else if pct >= PERCENTILE_ELEVATED {
        labels.push(format!("{name}:elevated"));
    }
}

/// Why a candidate kept its relevance position instead of being scored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unscored {
    /// Third-party code (`node_modules`, `.d.ts`): no history of its own here.
    Vendor,
    /// A file in an `add-root` root; history only covers the primary root.
    ExtraRoot,
    /// Project file without collected history (untracked, or newer than the
    /// last `hotspots --collect`).
    NoHistory,
}

impl Unscored {
    fn describe(self) -> &'static str {
        match self {
            Unscored::Vendor => "third-party code has no history in this repository",
            Unscored::ExtraRoot => "git history covers the primary root only",
            Unscored::NoHistory => {
                "no collected history for this file (untracked or newer than the last collection)"
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Component {
    pub name: &'static str,
    pub value: f64,
}

/// Evidence and position for one ranked result.
#[derive(Clone, Debug, Serialize)]
pub struct Dossier {
    /// Preset score 0..1; `null` when the candidate could not be scored.
    #[serde(serialize_with = "serialize_round3_option")]
    pub score: Option<f64>,
    /// The key the tier is sorted by: weighted blend of score and relevance.
    #[serde(serialize_with = "serialize_round3_option")]
    pub blended: Option<f64>,
    /// 1-based position in the plain relevance order of the pool; absent for
    /// files, whose path matches have no relevance order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relevance_rank: Option<usize>,
    pub tier: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unscored: Option<Unscored>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    /// Absent when the preset does not use history; `null` when it does but
    /// this result has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Option<HistoryDossier>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<Option<GraphDossier>>,
}

impl Dossier {
    fn render(&self, preset: Preset) -> Vec<String> {
        let mut lines = Vec::new();
        let position = match self.relevance_rank {
            Some(rank) => format!("relevance #{rank}, {}", self.tier),
            None => format!("match: {}", self.tier),
        };
        let head = match (self.score, self.unscored) {
            (Some(score), _) => {
                let parts: Vec<String> = self
                    .components
                    .iter()
                    .map(|component| format!("{} {:.2}", component.name, component.value))
                    .collect();
                format!(
                    "{} {:.2} = {} · {position}",
                    preset.as_str(),
                    score,
                    combine_label(preset, &parts),
                )
            }
            (None, Some(reason)) => format!(
                "{} not scored: {} · {position}",
                preset.as_str(),
                reason.describe(),
            ),
            (None, None) => position,
        };
        lines.push(head);
        if let Some(Some(history)) = &self.history {
            lines.push(history.render());
        }
        if let Some(Some(graph)) = &self.graph {
            lines.push(graph.render());
        }
        lines
    }
}

fn combine_label(preset: Preset, parts: &[String]) -> String {
    match preset {
        Preset::Proven => format!("mean({})", parts.join(", ")),
        Preset::Risky => parts.join(" × "),
        Preset::Hotspots | Preset::Central => parts.join(", "),
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

fn score(
    preset: Preset,
    history: Option<&FileHistory>,
    graph: Option<&GraphDossier>,
) -> Option<(f64, Vec<Component>)> {
    let hot = history.map(|history| history.hotspot.score_exact / 100.0);
    let components = match preset {
        Preset::Hotspots => vec![Component {
            name: "hotspot",
            value: hot?,
        }],
        Preset::Proven => {
            let history = history?;
            let graph = graph?;
            vec![
                Component {
                    name: "calm",
                    value: 1.0 - hot?,
                },
                Component {
                    name: "age",
                    value: history.hotspot.age_pct_exact / 100.0,
                },
                Component {
                    name: "idle",
                    value: history.idle_pct_exact / 100.0,
                },
                Component {
                    name: "used",
                    value: if graph.fan_in > 0 { 1.0 } else { 0.0 },
                },
            ]
        }
        Preset::Risky => {
            let graph = graph?;
            vec![
                Component {
                    name: "blast_radius",
                    value: graph.dependents_pct / 100.0,
                },
                Component {
                    name: "hotspot",
                    value: hot?,
                },
            ]
        }
        Preset::Central => {
            let graph = graph?;
            vec![Component {
                name: "pagerank",
                value: graph.pagerank_pct / 100.0,
            }]
        }
    };
    let value = match preset {
        Preset::Risky => components.iter().map(|c| c.value).product(),
        _ => components.iter().map(|c| c.value).sum::<f64>() / components.len() as f64,
    };
    let components = components
        .into_iter()
        .map(|component| Component {
            name: component.name,
            value: round3(component.value),
        })
        .collect();
    Some((value, components))
}

/// History for a result's file, or why there is none. Collected history
/// only describes the primary root's working tree: an attached root's
/// `src/a.rs` is a different file with the same relative path.
fn history_for<'a>(
    ctx: &'a RankContext,
    path: &str,
    primary_root: bool,
) -> std::result::Result<Option<&'a FileHistory>, Unscored> {
    let Some(snapshot) = &ctx.history else {
        return Ok(None);
    };
    if !primary_root {
        return Err(Unscored::ExtraRoot);
    }
    snapshot
        .files
        .get(path)
        .map(Some)
        .ok_or(Unscored::NoHistory)
}

struct Candidate<T> {
    item: T,
    relevance_rank: usize,
    tier: u8,
    dossier: Dossier,
}

/// How relevance enters the order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grading {
    /// Symbols: tiers are hard boundaries and, inside a tier, the relevance
    /// term decays with the candidate's position. The decay follows the
    /// position rather than the tier's size: the first few positions of a
    /// tier are near-equivalent (exact matches are ordered by name length and
    /// path, partial ones by bm25), so a small tier must not turn one step
    /// into a large penalty.
    Positional,
    /// Files: every candidate contains the query in its path and the plain
    /// order is alphabetical, so the only relevance there is where the match
    /// sits (file stem, file name, directory). That tier is the relevance
    /// term itself, `1 / (1 + tier)`, not a boundary: otherwise a test file
    /// named after the query, scored 0, would outrank the service everyone
    /// depends on because its match is in the directory part.
    PathTier,
}

/// Order candidates: scored ones by the blended key, unscored ones after the
/// scored candidates of their tier in relevance order.
///
/// Third-party candidates go after every project candidate, whatever their
/// tier: each preset asks about the project's own code, and a library
/// definition has neither project history nor graph edges (`graph build`
/// never targets installed packages). Without this, an exact-name match in
/// a dozen vendored copies of a type declaration fills the page before the
/// first project result the preset could say anything about.
fn order<T>(candidates: &mut [Candidate<T>], grading: Grading) {
    let vendor = |candidate: &Candidate<T>| candidate.dossier.unscored == Some(Unscored::Vendor);
    let mut seen: HashMap<u8, usize> = HashMap::new();
    for candidate in candidates.iter_mut() {
        if vendor(candidate) {
            continue;
        }
        let position = seen.entry(candidate.tier).or_default();
        let relevance = match grading {
            Grading::Positional => 1.0 / (1.0 + *position as f64 / RELEVANCE_HALF),
            Grading::PathTier => 1.0 / (1.0 + f64::from(candidate.tier)),
        };
        *position += 1;
        if let Some(score) = candidate.dossier.score {
            candidate.dossier.blended =
                Some(PRESET_WEIGHT * score + (1.0 - PRESET_WEIGHT) * relevance);
        }
    }
    let hard_tier = |candidate: &Candidate<T>| match grading {
        Grading::Positional => candidate.tier,
        Grading::PathTier => 0,
    };
    candidates.sort_by(|left, right| {
        vendor(left)
            .cmp(&vendor(right))
            .then_with(|| hard_tier(left).cmp(&hard_tier(right)))
            .then_with(|| match (left.dossier.blended, right.dossier.blended) {
                (Some(a), Some(b)) => b.total_cmp(&a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| left.tier.cmp(&right.tier))
            .then_with(|| left.relevance_rank.cmp(&right.relevance_rank))
    });
}

// ---------------------------------------------------------------------------
// Tiers
// ---------------------------------------------------------------------------

const SYMBOL_TIERS: [&str; 4] = ["exact_name", "exact_name_folded", "name", "signature"];
const FILE_TIERS: [&str; 3] = ["file_stem", "file_name", "directory"];

/// Relevance tier of a symbol hit, mirroring what the plain order ranks
/// first: the name is a term (case-sensitively, then folded; fuzzy search
/// does not tell case apart), a word of the name starts with a term (what
/// FTS prefix matching found; a substring under `--fuzzy`), or only the
/// signature matched.
fn symbol_tier(result: &SearchResult, terms: &[&str], fuzzy: bool) -> u8 {
    let name = result.name.as_str();
    let terms: Vec<&str> = terms.iter().map(|t| t.trim_end_matches('*')).collect();
    if !fuzzy && terms.contains(&name) {
        return 0;
    }
    let folded = name.to_lowercase();
    let folded_terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    if folded_terms.contains(&folded) {
        return if fuzzy { 0 } else { 1 };
    }
    let named = if fuzzy {
        let display = result.display_name().to_lowercase();
        folded_terms
            .iter()
            .any(|term| folded.contains(term.as_str()) || display.contains(term.as_str()))
    } else {
        folded.split(|c: char| !c.is_alphanumeric()).any(|word| {
            folded_terms
                .iter()
                .any(|term| word.starts_with(term.as_str()))
        })
    };
    if named {
        2
    } else {
        3
    }
}

/// Relevance tier of a path hit: the file name without extension is a term,
/// the file name contains one, or only a directory does.
fn file_tier(path: &str, terms: &[&str]) -> u8 {
    let file_name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let stem = file_name
        .split('.')
        .next()
        .unwrap_or(&file_name)
        .to_string();
    let terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    if terms.contains(&stem) {
        0
    } else if terms.iter().any(|term| file_name.contains(term.as_str())) {
        1
    } else {
        2
    }
}

// ---------------------------------------------------------------------------
// Ranking entry points
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RankedSymbol {
    #[serde(flatten)]
    pub result: SearchResult,
    pub rank: Option<Dossier>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RankedFile {
    pub path: String,
    #[serde(skip_serializing)]
    pub root_path: Option<String>,
    pub rank: Option<Dossier>,
}

fn blank_dossier(ctx: &RankContext, relevance_rank: Option<usize>, tier: &'static str) -> Dossier {
    Dossier {
        score: None,
        blended: None,
        relevance_rank,
        tier,
        unscored: None,
        components: Vec::new(),
        history: ctx.preset.needs_history().then_some(None),
        graph: ctx.preset.needs_graph().then_some(None),
    }
}

/// Re-rank a relevance-ordered symbol pool and keep the first `limit`.
/// When the preset is not applied, the pool keeps its order and carries no
/// dossier.
pub fn rank_symbols(
    conn: &Connection,
    ctx: &RankContext,
    resolver: &PathResolver,
    pool: Vec<(i64, SearchResult)>,
    terms: &[&str],
    fuzzy: bool,
    limit: usize,
) -> Result<Vec<RankedSymbol>> {
    if !ctx.applied() {
        return Ok(pool
            .into_iter()
            .take(limit)
            .map(|(_, result)| RankedSymbol { result, rank: None })
            .collect());
    }
    let ids: Vec<i64> = pool.iter().map(|(id, _)| *id).collect();
    let metrics = if ctx.graph.is_some() {
        db::load_symbol_graph_metrics(conn, &ids)?
    } else {
        HashMap::new()
    };
    let mut candidates: Vec<Candidate<SearchResult>> = Vec::with_capacity(pool.len());
    for (position, (id, result)) in pool.into_iter().enumerate() {
        let tier = symbol_tier(&result, terms, fuzzy);
        let mut dossier = blank_dossier(ctx, Some(position + 1), SYMBOL_TIERS[tier as usize]);
        if db::is_vendor_path(&result.path) {
            dossier.unscored = Some(Unscored::Vendor);
        } else {
            let graph = ctx
                .graph
                .as_ref()
                .map(|population| GraphDossier::build(metrics.get(&id), population, None));
            let primary = resolver.is_primary_root(result.root_path.as_deref());
            fill(ctx, &mut dossier, &result.path, primary, graph);
        }
        candidates.push(Candidate {
            item: result,
            relevance_rank: position + 1,
            tier,
            dossier,
        });
    }
    order(&mut candidates, Grading::Positional);
    Ok(candidates
        .into_iter()
        .take(limit)
        .map(|candidate| RankedSymbol {
            result: candidate.item,
            rank: Some(candidate.dossier),
        })
        .collect())
}

/// Re-rank a pool of path matches and keep the first `limit`.
pub fn rank_files(
    conn: &Connection,
    ctx: &RankContext,
    resolver: &PathResolver,
    pool: Vec<FileResult>,
    terms: &[&str],
    limit: usize,
) -> Result<Vec<RankedFile>> {
    if !ctx.applied() {
        return Ok(pool
            .into_iter()
            .take(limit)
            .map(|file| RankedFile {
                path: file.path,
                root_path: file.root_path,
                rank: None,
            })
            .collect());
    }
    let strongest = if ctx.graph.is_some() {
        strongest_symbols(conn, &pool)?
    } else {
        HashMap::new()
    };
    let mut candidates: Vec<Candidate<FileResult>> = Vec::with_capacity(pool.len());
    for (position, file) in pool.into_iter().enumerate() {
        let tier = file_tier(&file.path, terms);
        let mut dossier = blank_dossier(ctx, None, FILE_TIERS[tier as usize]);
        if db::is_vendor_path(&file.path) {
            dossier.unscored = Some(Unscored::Vendor);
        } else {
            let graph = ctx.graph.as_ref().map(|population| {
                let key = (file.root_path.clone(), file.path.clone());
                match strongest.get(&key) {
                    Some(row) => GraphDossier::build(
                        Some(&row.metrics),
                        population,
                        Some(StrongestSymbol {
                            name: row.name.clone(),
                            kind: row.kind.clone(),
                            line: row.line,
                        }),
                    ),
                    None => GraphDossier {
                        granularity: "file",
                        ..GraphDossier::build(None, population, None)
                    },
                }
            });
            let primary = resolver.is_primary_root(file.root_path.as_deref());
            fill(ctx, &mut dossier, &file.path, primary, graph);
        }
        candidates.push(Candidate {
            item: file,
            relevance_rank: position + 1,
            tier,
            dossier,
        });
    }
    order(&mut candidates, Grading::PathTier);
    Ok(candidates
        .into_iter()
        .take(limit)
        .map(|candidate| RankedFile {
            path: candidate.item.path,
            root_path: candidate.item.root_path,
            rank: Some(candidate.dossier),
        })
        .collect())
}

fn fill(
    ctx: &RankContext,
    dossier: &mut Dossier,
    path: &str,
    primary_root: bool,
    graph: Option<GraphDossier>,
) {
    let history = match history_for(ctx, path, primary_root) {
        Ok(history) => history,
        Err(reason) => {
            dossier.unscored = Some(reason);
            None
        }
    };
    if dossier.history.is_some() {
        dossier.history = Some(history.map(HistoryDossier::from));
    }
    if dossier.unscored.is_none() {
        if let Some((value, components)) = score(ctx.preset, history, graph.as_ref()) {
            dossier.score = Some(value);
            dossier.components = components;
        }
    }
    if dossier.graph.is_some() {
        dossier.graph = Some(graph);
    }
}

/// Per file (keyed by root and path), the symbol with the highest PageRank.
fn strongest_symbols(
    conn: &Connection,
    pool: &[FileResult],
) -> Result<HashMap<(Option<String>, String), db::FileSymbolMetrics>> {
    let mut paths: Vec<&str> = pool.iter().map(|file| file.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    let mut best: HashMap<(Option<String>, String), db::FileSymbolMetrics> = HashMap::new();
    for row in db::load_file_symbol_metrics(conn, &paths)? {
        let key = (row.root_path.clone(), row.path.clone());
        let replace = match best.get(&key) {
            None => true,
            Some(current) => row
                .metrics
                .pagerank
                .total_cmp(&current.metrics.pagerank)
                .then_with(|| current.line.cmp(&row.line))
                .is_gt(),
        };
        if replace {
            best.insert(key, row);
        }
    }
    Ok(best)
}

// ---------------------------------------------------------------------------
// Report header
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct HistorySummary {
    pub granularity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<i64>,
    pub commits_analyzed: usize,
    pub files_with_history: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct GraphSummary {
    pub granularity: &'static str,
    pub built: bool,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_at: Option<i64>,
}

/// How many project candidates were re-ranked (third-party ones are
/// appended after them unscored and not counted).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PoolSummary {
    pub symbols: usize,
    pub files: usize,
    /// `--exclude-tests`: test files were left out of the pool and the totals.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tests_excluded: bool,
}

/// The `rank` object of a ranked search report.
#[derive(Clone, Debug, Serialize)]
pub struct RankSummary {
    pub preset: Preset,
    pub applied: bool,
    pub formula: &'static str,
    pub missing: Vec<MissingSignal>,
    pub warnings: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<HistorySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphSummary>,
    pub pool: PoolSummary,
    pub preset_weight: f64,
    pub ranked_sections: [&'static str; 2],
}

impl RankContext {
    pub fn summary(&self, pool: PoolSummary) -> RankSummary {
        RankSummary {
            preset: self.preset,
            applied: self.applied(),
            formula: self.preset.formula(),
            missing: self.missing.clone(),
            warnings: self.warnings.clone(),
            history: self.history.as_ref().map(|snapshot| HistorySummary {
                granularity: "file",
                head: snapshot.head.clone(),
                collected_at: snapshot.collected_at,
                commits_analyzed: snapshot.commits_analyzed,
                files_with_history: snapshot.files.len(),
            }),
            graph: self.graph_state.as_ref().map(|state| GraphSummary {
                granularity: "symbol",
                built: state.built,
                stale: state.stale,
                built_at: state.built_at,
            }),
            pool,
            preset_weight: PRESET_WEIGHT,
            ranked_sections: ["files", "symbols"],
        }
    }
}

/// Header lines for text output, printed under the search title.
pub fn render_header(summary: &RankSummary) -> Vec<String> {
    let mut lines = Vec::new();
    if !summary.applied {
        let needs: Vec<String> = summary
            .missing
            .iter()
            .map(|missing| format!("{} (run '{}')", missing.reason, missing.command))
            .collect();
        lines.push(
            format!(
                "Ranking '{}' NOT applied: {}. Results below are in plain relevance order.",
                summary.preset.as_str(),
                needs.join("; ")
            )
            .yellow()
            .to_string(),
        );
        return lines;
    }
    lines.push(
        format!("{} = {}.", summary.preset.as_str(), summary.formula)
            .dimmed()
            .to_string(),
    );
    let now_ms = now_millis();
    let mut evidence = Vec::new();
    if let Some(history) = &summary.history {
        evidence.push(format!(
            "file history of {} commits (HEAD {}{}); a file's history is shared by every symbol in it",
            history.commits_analyzed,
            history.head.as_deref().map(short_sha).unwrap_or("?"),
            history
                .collected_at
                .map(|at| format!(", collected {}", ago(now_ms, at)))
                .unwrap_or_default()
        ));
    }
    if let Some(graph) = &summary.graph {
        evidence.push(format!(
            "symbol graph{}",
            graph
                .built_at
                .map(|at| format!(" built {}", ago(now_ms, at)))
                .unwrap_or_default()
        ));
    }
    if !evidence.is_empty() {
        lines.push(
            format!("Evidence: {}.", evidence.join("; "))
                .dimmed()
                .to_string(),
        );
    }
    lines.push(
        format!(
            "Re-ranked the top {} project symbol(s) and {} project file(s) by relevance{}; an exact name stays above a partial one, third-party code goes last.",
            summary.pool.symbols,
            summary.pool.files,
            if summary.pool.tests_excluded {
                ", test files left out"
            } else {
                ""
            }
        )
        .dimmed()
        .to_string(),
    );
    for warning in &summary.warnings {
        lines.push(format!("Warning: {warning}.").yellow().to_string());
    }
    lines
}

pub fn render_dossier(dossier: &Dossier, preset: Preset) -> Vec<String> {
    dossier.render(preset)
}

fn ago(now_ms: i64, at_ms: i64) -> String {
    let days = (now_ms - at_ms).max(0) / 1000 / SECONDS_PER_DAY;
    match days {
        0 => "today".to_string(),
        1 => "1 day ago".to_string(),
        days => format!("{days} days ago"),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            qualified_name: None,
            kind: "class".to_string(),
            line: 1,
            signature: None,
            path: "app/a.rb".to_string(),
            root_path: None,
        }
    }

    #[test]
    fn symbol_tiers_follow_what_was_typed() {
        let terms = ["Merge"];
        assert_eq!(symbol_tier(&symbol("Merge"), &terms, false), 0);
        assert_eq!(symbol_tier(&symbol("merge"), &terms, false), 1);
        assert_eq!(
            symbol_tier(&symbol("Applicant::MergeService"), &terms, false),
            2
        );
        assert_eq!(symbol_tier(&symbol("merge_data"), &terms, false), 2);
        assert_eq!(symbol_tier(&symbol("AutoMerge"), &terms, false), 3);
        assert_eq!(symbol_tier(&symbol("AutoMerge"), &terms, true), 2);
        assert_eq!(symbol_tier(&symbol("MERGE"), &terms, true), 0);
    }

    #[test]
    fn file_tiers_prefer_the_file_name() {
        let terms = ["merge"];
        assert_eq!(file_tier("app/services/merge.rb", &terms), 0);
        assert_eq!(file_tier("app/services/merge_service.rb", &terms), 1);
        assert_eq!(file_tier("app/services/merge/base.rb", &terms), 2);
    }

    #[test]
    fn graph_percentiles_rank_against_referenced_symbols_only() {
        let population = GraphPopulation {
            fan_in_files: vec![1.0, 1.0, 2.0, 10.0],
            dependents: vec![1.0, 2.0, 3.0, 500.0],
            pageranks: vec![1.0, 2.0, 3.0, 4.0],
        };
        let unreferenced = SymbolGraphMetrics {
            fan_in_ambiguous: 4,
            fan_out: 2,
            ..SymbolGraphMetrics::default()
        };
        let dossier = GraphDossier::build(Some(&unreferenced), &population, None);
        assert_eq!(dossier.dependents_pct, 0.0);
        assert_eq!(dossier.pagerank_pct, 0.0);
        assert_eq!(dossier.labels, vec!["callers:unresolved".to_string()]);

        let hub = SymbolGraphMetrics {
            fan_in: 12,
            fan_in_files: 10,
            dependents: 500,
            pagerank: 4.0,
            ..SymbolGraphMetrics::default()
        };
        let dossier = GraphDossier::build(Some(&hub), &population, None);
        assert_eq!(dossier.dependents_pct, 87.5);
        assert!(dossier.labels.contains(&"dependents:elevated".to_string()));
    }

    #[test]
    fn order_puts_vendor_after_every_project_candidate() {
        let vendor = Dossier {
            score: None,
            blended: None,
            relevance_rank: Some(1),
            tier: "exact_name",
            unscored: Some(Unscored::Vendor),
            components: Vec::new(),
            history: None,
            graph: None,
        };
        let project = Dossier {
            score: Some(0.1),
            unscored: None,
            relevance_rank: Some(2),
            tier: "signature",
            ..vendor.clone()
        };
        let mut candidates = vec![
            Candidate {
                item: "vendor-exact",
                relevance_rank: 1,
                tier: 0,
                dossier: vendor,
            },
            Candidate {
                item: "project-signature",
                relevance_rank: 2,
                tier: 3,
                dossier: project,
            },
        ];
        order(&mut candidates, Grading::Positional);
        assert_eq!(candidates[0].item, "project-signature");
    }

    #[test]
    fn path_tiers_weigh_in_but_do_not_bound_the_order() {
        let dossier = |score: f64| Dossier {
            score: Some(score),
            blended: None,
            relevance_rank: None,
            tier: "file_stem",
            unscored: None,
            components: Vec::new(),
            history: None,
            graph: None,
        };
        let candidate = |item: &'static str, rank: usize, tier: u8, score: f64| Candidate {
            item,
            relevance_rank: rank,
            tier,
            dossier: dossier(score),
        };
        let mut candidates = vec![
            candidate("stem-cold", 1, 0, 0.0),
            candidate("stem-warm", 2, 0, 0.5),
            candidate("name-hot", 3, 1, 0.9),
            candidate("name-warm", 4, 1, 0.52),
        ];
        order(&mut candidates, Grading::PathTier);
        let names: Vec<&str> = candidates.iter().map(|c| c.item).collect();
        assert_eq!(
            names,
            vec!["name-hot", "stem-warm", "name-warm", "stem-cold"]
        );
    }

    #[test]
    fn order_keeps_tiers_and_puts_unscored_last_within_tier() {
        let dossier = |score: Option<f64>| Dossier {
            score,
            blended: None,
            relevance_rank: None,
            tier: "name",
            unscored: None,
            components: Vec::new(),
            history: None,
            graph: None,
        };
        let candidate = |item: &'static str, rank: usize, tier: u8, score: Option<f64>| Candidate {
            item,
            relevance_rank: rank,
            tier,
            dossier: dossier(score),
        };
        let mut candidates = vec![
            candidate("exact-low", 1, 0, Some(0.1)),
            candidate("partial-unscored", 2, 2, None),
            candidate("partial-high", 3, 2, Some(0.9)),
            candidate("partial-low", 4, 2, Some(0.2)),
        ];
        order(&mut candidates, Grading::Positional);
        let names: Vec<&str> = candidates.iter().map(|c| c.item).collect();
        assert_eq!(
            names,
            vec![
                "exact-low",
                "partial-high",
                "partial-low",
                "partial-unscored"
            ]
        );
    }
}
