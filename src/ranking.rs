use std::collections::BTreeSet;

use serde_json::json;
use sha2::{Digest, Sha256};

mod decision;
mod explanation;
mod output;
mod pagerank;

use crate::{
    model::Issue,
    operational::{ExecutionScope, OperationalGraph},
    priority::{PriorityComparison, PriorityState},
    working_graph::WorkingGraph,
};
use decision::{PriorityProfile, RankingMode};
pub(crate) use output::NextAnalysis;
use output::{ExactHorizonOneResult, NextSummary};
use pagerank::PageRank;

pub(crate) const POLICY_VERSION: &str = "next/v1";
pub(crate) const OUTPUT_SCHEMA_VERSION: &str = "grit.next/v1";
pub(crate) const HORIZON: u8 = 1;
const ALTERNATIVE_LIMIT: usize = 10;

struct EvaluatedCandidate<'a> {
    issue: &'a Issue,
    unlocks: Vec<&'a Issue>,
    priority_profile: PriorityProfile,
    p0_unlock_count: usize,
    step_priority: PriorityComparison,
    pagerank_bucket: Option<u64>,
}

pub(crate) fn analyze(working: &WorkingGraph<'_>, scope: ExecutionScope<'_>) -> NextAnalysis {
    let replica = working.replica();
    let graph = OperationalGraph::prepare(replica);
    let ready = graph.analyze_ready(scope);
    let pagerank = PageRank::calculate(&graph);
    let executable_p0: Vec<_> = ready
        .executable
        .iter()
        .copied()
        .filter(|issue| priority(working, issue) == PriorityComparison::P0)
        .collect();
    let route_candidates = if executable_p0.is_empty() {
        p0_route_candidates(working, &graph, &ready.executable)
    } else {
        BTreeSet::new()
    };
    let (mode, candidates): (RankingMode, Vec<_>) = if !executable_p0.is_empty() {
        (RankingMode::P0Ready, executable_p0)
    } else if !route_candidates.is_empty() {
        (
            RankingMode::P0Route,
            ready
                .executable
                .iter()
                .copied()
                .filter(|issue| route_candidates.contains(&issue.number))
                .collect(),
        )
    } else if ready.executable.is_empty() {
        (RankingMode::None, Vec::new())
    } else {
        (RankingMode::Normal, ready.executable.clone())
    };

    let candidate_count = candidates.len();
    let mut ranking_provenance_context =
        p0_gate_provenance_numbers(working, &graph, &ready.executable);
    for candidate in &candidates {
        ranking_provenance_context.push(candidate.number);
        ranking_provenance_context.extend(graph.immediate_unlocks_for(candidate.number));
    }
    let evaluated = select_top_candidates(
        candidates
            .into_iter()
            .map(|issue| evaluate_candidate(working, &graph, issue, pagerank.as_ref())),
        mode,
        ALTERNATIVE_LIMIT + 1,
    );
    let decisive = evaluated
        .first()
        .zip(evaluated.get(1))
        .map(|(winner, runner_up)| decision::compare(winner, runner_up, mode).decisive);
    let comparison = decisive.zip(evaluated.first().zip(evaluated.get(1))).map(
        |(decision, (winner, runner_up))| {
            explanation::evidence(
                decision,
                winner,
                runner_up,
                working,
                &ranking_provenance_context,
            )
        },
    );
    let close_call = decisive.is_some_and(decision::DecisiveComparison::is_close_call);
    let mut comparison_reason = decisive.map(explanation::reason);
    let executable_p0_count = ready
        .executable
        .iter()
        .filter(|issue| priority(working, issue) == PriorityComparison::P0)
        .count();

    let mut ranked_results = evaluated.into_iter().enumerate().map(|(index, candidate)| {
        let mut reasons = explanation::mode_reasons(mode, executable_p0_count, &candidate);
        if index == 0 {
            if let Some(reason) = comparison_reason.take() {
                reasons.push(reason);
            } else if candidate_count == 1 && reasons.is_empty() {
                reasons.push(explanation::only_candidate_reason(candidate_count));
            }
        }
        output::candidate_output(
            candidate,
            mode,
            working,
            &ranking_provenance_context,
            reasons,
        )
    });
    let recommendation = ranked_results.next();
    let alternatives: Vec<_> = ranked_results.take(ALTERNATIVE_LIMIT).collect();
    let summary = NextSummary::from_graph(&ready, candidate_count, &graph);
    NextAnalysis::exact_horizon_one(ExactHorizonOneResult {
        input_hash: effective_input_hash(working, scope),
        pending: working.is_pending(),
        pending_operation_ids: working.operation_ids(),
        mode,
        pagerank_available: pagerank.is_some(),
        recommendation,
        alternatives,
        comparison_to_runner_up: comparison,
        close_call,
        summary,
    })
}

fn select_top_candidates<'a>(
    candidates: impl IntoIterator<Item = EvaluatedCandidate<'a>>,
    mode: RankingMode,
    limit: usize,
) -> Vec<EvaluatedCandidate<'a>> {
    let mut best = Vec::with_capacity(limit);
    for candidate in candidates {
        let position = best
            .iter()
            .position(|current| {
                decision::compare(&candidate, current, mode).ordering == std::cmp::Ordering::Greater
            })
            .unwrap_or(best.len());
        if position < limit {
            best.insert(position, candidate);
            if best.len() > limit {
                best.pop();
            }
        }
    }
    best
}

fn p0_route_candidates(
    working: &WorkingGraph<'_>,
    graph: &OperationalGraph<'_>,
    executable: &[&Issue],
) -> BTreeSet<u64> {
    executable
        .iter()
        .filter(|candidate| {
            graph
                .immediate_unlocks_for(candidate.number)
                .iter()
                .filter_map(|number| graph.issue(*number))
                .any(|target| priority(working, target) == PriorityComparison::P0)
        })
        .map(|candidate| candidate.number)
        .collect()
}

fn evaluate_candidate<'a>(
    working: &WorkingGraph<'_>,
    graph: &OperationalGraph<'a>,
    issue: &'a Issue,
    pagerank: Option<&PageRank>,
) -> EvaluatedCandidate<'a> {
    let unlocks: Vec<_> = graph
        .immediate_unlocks_for(issue.number)
        .iter()
        .filter_map(|dependent| graph.issue(*dependent))
        .collect();
    let mut priority_profile = PriorityProfile::default();
    let mut p0_unlock_count = 0;
    for unlocked in &unlocks {
        let unlocked_priority = priority(working, unlocked);
        if unlocked_priority == PriorityComparison::P0 {
            p0_unlock_count += 1;
        } else {
            priority_profile.record(unlocked_priority);
        }
    }
    EvaluatedCandidate {
        issue,
        unlocks,
        priority_profile,
        p0_unlock_count,
        step_priority: priority(working, issue),
        pagerank_bucket: pagerank.and_then(|pagerank| pagerank.bucket(issue.number)),
    }
}

fn priority(working: &WorkingGraph<'_>, issue: &Issue) -> PriorityComparison {
    working.priority(issue).comparison()
}

fn p0_gate_provenance_numbers(
    working: &WorkingGraph<'_>,
    graph: &OperationalGraph<'_>,
    executable: &[&Issue],
) -> Vec<u64> {
    let mut inspected = BTreeSet::new();
    for issue in executable {
        inspected.insert(issue.number);
        inspected.extend(graph.immediate_unlocks_for(issue.number).iter().copied());
    }
    inspected
        .into_iter()
        .filter(|number| {
            graph.issue(*number).is_some_and(|issue| {
                let base = PriorityState::from_issue_labels(&issue.labels).comparison();
                (base == PriorityComparison::P0)
                    != (priority(working, issue) == PriorityComparison::P0)
            })
        })
        .collect()
}

fn effective_input_hash(working: &WorkingGraph<'_>, scope: ExecutionScope<'_>) -> String {
    let (mode, assignee) = scope.hash_key();
    let input = json!({
        "schema_version": "grit.working-input/v1",
        "working_graph_hash": working.input_hash(),
        "execution_scope": {
            "mode": mode,
            "assignee": assignee,
        }
    });
    let canonical = serde_json::to_vec(&input).expect("effective input hash is serializable");
    hex::encode(Sha256::digest(canonical))
}
