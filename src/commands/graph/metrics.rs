//! Graph metrics computed once per `graph build` over resolved edges.

use std::collections::{HashMap, HashSet};

use super::Confidence;
use crate::db::{SymbolEdgeRow, SymbolGraphMetrics};

const PAGERANK_DAMPING: f64 = 0.85;
const PAGERANK_MAX_ITERATIONS: usize = 100;
const PAGERANK_TOLERANCE: f64 = 1e-10;

/// Fan-in/out, depth-limited transitive dependents and PageRank.
///
/// Every metric except the `*_ambiguous` counters walks resolved edges only.
/// Measured on a 40k-file Ruby/TypeScript monorepo, the true target was among
/// an ambiguous reference's candidates in under a fifth of sampled cases, and
/// weighting ambiguous edges 1/k let `present?`, `blank?`, `merge`, `send` and
/// DSL helpers displace real base classes from the PageRank top 30.
pub fn compute_metrics(
    edges: &[SymbolEdgeRow],
    file_of: &HashMap<i64, u32>,
    dependents_depth: usize,
) -> Vec<SymbolGraphMetrics> {
    let mut dense: HashMap<i64, usize> = HashMap::new();
    let mut ids: Vec<i64> = Vec::new();
    for edge in edges {
        for id in [edge.source_id, edge.target_id] {
            dense.entry(id).or_insert_with(|| {
                ids.push(id);
                ids.len() - 1
            });
        }
    }
    let n = ids.len();
    let mut metrics: Vec<SymbolGraphMetrics> = ids
        .iter()
        .map(|&id| SymbolGraphMetrics {
            symbol_id: id,
            ..SymbolGraphMetrics::default()
        })
        .collect();
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); n];
    for edge in edges {
        let source = dense[&edge.source_id];
        let target = dense[&edge.target_id];
        if Confidence::from_code(edge.confidence).is_resolved() {
            outgoing[source].push(target);
            incoming[target].push(source);
            metrics[source].fan_out += 1;
            metrics[target].fan_in += 1;
        } else {
            metrics[source].fan_out_ambiguous += 1;
            metrics[target].fan_in_ambiguous += 1;
        }
    }

    for (target, sources) in incoming.iter().enumerate() {
        let files: HashSet<u32> = sources
            .iter()
            .filter_map(|&source| file_of.get(&ids[source]).copied())
            .collect();
        metrics[target].fan_in_files = files.len() as u32;
    }

    let dependents = count_dependents(&incoming, dependents_depth);
    for (index, count) in dependents.into_iter().enumerate() {
        metrics[index].dependents = count;
    }

    let ranks = pagerank(&outgoing);
    let percentiles = percentile_ranks(&ranks);
    for index in 0..n {
        metrics[index].pagerank = round_to(ranks[index] * n as f64, 6);
        metrics[index].pagerank_pct = round_to(percentiles[index], 2);
    }
    metrics
}

/// Distinct nodes that reach each node within `depth` hops of `incoming`.
fn count_dependents(incoming: &[Vec<usize>], depth: usize) -> Vec<u32> {
    let n = incoming.len();
    let mut counts = vec![0u32; n];
    let mut stamp = vec![usize::MAX; n];
    let mut frontier: Vec<usize> = Vec::new();
    let mut next: Vec<usize> = Vec::new();
    for start in 0..n {
        if incoming[start].is_empty() {
            continue;
        }
        stamp[start] = start;
        frontier.clear();
        frontier.push(start);
        let mut reached = 0u32;
        for _ in 0..depth {
            next.clear();
            for &node in &frontier {
                for &source in &incoming[node] {
                    if stamp[source] != start {
                        stamp[source] = start;
                        reached += 1;
                        next.push(source);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            std::mem::swap(&mut frontier, &mut next);
        }
        counts[start] = reached;
    }
    counts
}

/// Classic PageRank over `outgoing` (dependency direction), uniform teleport,
/// dangling mass redistributed uniformly. Returns a distribution summing to 1.
fn pagerank(outgoing: &[Vec<usize>]) -> Vec<f64> {
    let n = outgoing.len();
    if n == 0 {
        return Vec::new();
    }
    let uniform = 1.0 / n as f64;
    let mut rank = vec![uniform; n];
    let mut next = vec![0.0; n];
    for _ in 0..PAGERANK_MAX_ITERATIONS {
        let mut dangling = 0.0;
        next.iter_mut().for_each(|value| *value = 0.0);
        for (node, targets) in outgoing.iter().enumerate() {
            if targets.is_empty() {
                dangling += rank[node];
            } else {
                let share = rank[node] / targets.len() as f64;
                for &target in targets {
                    next[target] += share;
                }
            }
        }
        let base = (1.0 - PAGERANK_DAMPING) * uniform + PAGERANK_DAMPING * dangling * uniform;
        let mut delta = 0.0;
        for node in 0..n {
            let value = base + PAGERANK_DAMPING * next[node];
            delta += (value - rank[node]).abs();
            rank[node] = value;
        }
        if delta < PAGERANK_TOLERANCE {
            break;
        }
    }
    rank
}

/// Midrank percentile of every value within the population, 0..100.
fn percentile_ranks(values: &[f64]) -> Vec<f64> {
    let count = values.len();
    if count == 0 {
        return Vec::new();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    values
        .iter()
        .map(|value| {
            let less = sorted.partition_point(|candidate| candidate < value);
            let not_greater = sorted.partition_point(|candidate| candidate <= value);
            100.0 * (less as f64 + 0.5 * (not_greater - less) as f64) / count as f64
        })
        .collect()
}

fn round_to(value: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagerank_favours_depended_upon_nodes() {
        let outgoing = vec![vec![2], vec![2], vec![], vec![2]];
        let ranks = pagerank(&outgoing);
        let sum: f64 = ranks.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9);
        assert!(ranks[2] > ranks[0] && ranks[2] > ranks[1] && ranks[2] > ranks[3]);
    }

    #[test]
    fn dependents_are_depth_limited() {
        // 3 -> 2 -> 1 -> 0 (dependency direction), incoming lists reversed.
        let incoming = vec![vec![1], vec![2], vec![3], vec![]];
        assert_eq!(count_dependents(&incoming, 1), vec![1, 1, 1, 0]);
        assert_eq!(count_dependents(&incoming, 3), vec![3, 2, 1, 0]);
    }
}
