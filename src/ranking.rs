use serde_json::json;
use sha2::{Digest, Sha256};

mod decision;
mod explanation;
mod output;
mod pagerank;
mod search;

use crate::{
    model::Issue,
    operational::{ExecutionScope, IssueState, OperationalGraph},
    priority::PriorityComparison,
    working_graph::WorkingGraph,
};
use decision::{PriorityProfile, RankingMode, StepPriority};
pub(crate) use output::NextAnalysis;
use output::{NextResult, NextSummary};
use pagerank::PageRank;

pub(crate) const POLICY_VERSION: &str = "next/v1";
pub(crate) const OUTPUT_SCHEMA_VERSION: &str = "grit.next/v1";
pub(crate) const DEFAULT_HORIZON: u8 = 3;
pub(crate) const MIN_HORIZON: u8 = 1;
pub(crate) const MAX_HORIZON: u8 = 3;
const ALTERNATIVE_LIMIT: usize = 10;
const STATE_BUDGET: usize = 8_192;

#[derive(Clone)]
struct EvaluatedStep<'a> {
    issue: &'a Issue,
    mode: RankingMode,
}

struct EvaluatedCandidate<'a> {
    issue: &'a Issue,
    steps: Vec<EvaluatedStep<'a>>,
    unlocks: Vec<&'a Issue>,
    priority_profile: PriorityProfile,
    unlock_curve: Vec<usize>,
    p0_curve: Vec<usize>,
    step_priorities: Vec<StepPriority>,
    pagerank_bucket: Option<u64>,
}

pub(crate) fn analyze(
    working: &WorkingGraph<'_>,
    scope: ExecutionScope<'_>,
    horizon: u8,
) -> NextAnalysis {
    let replica = working.replica();
    let graph = OperationalGraph::prepare(replica);
    let ready = graph.analyze_ready(scope);
    let pagerank = PageRank::calculate(&graph);
    let deferred_critical_routes = horizon > 1
        && replica.issues.iter().any(|issue| {
            IssueState::parse(&issue.state) == IssueState::Open
                && priority(working, issue) == PriorityComparison::P0
                && !graph.is_ready(issue.number)
        });
    let search = search::evaluate(
        working,
        &graph,
        scope,
        pagerank.as_ref(),
        horizon,
        STATE_BUDGET,
        deferred_critical_routes,
    );
    let mode = search.mode;
    let candidate_count = search.candidate_count;
    let search_complete = search.truncated_by.is_empty();
    let truncated_by = search.truncated_by;
    let ranking_provenance_context: Vec<_> = search.provenance_numbers.into_iter().collect();
    let evaluated = select_top_candidates(search.candidates, mode, ALTERNATIVE_LIMIT + 1);
    let decisive = evaluated
        .first()
        .zip(evaluated.get(1))
        .map(|(winner, runner_up)| decision::compare(winner, runner_up, mode).decisive);
    let comparison = decisive
        .as_ref()
        .zip(evaluated.first().zip(evaluated.get(1)))
        .map(|(decision, (winner, runner_up))| {
            explanation::evidence(
                decision,
                winner,
                runner_up,
                working,
                &ranking_provenance_context,
            )
        });
    let close_call = decisive
        .as_ref()
        .is_some_and(decision::DecisiveComparison::is_close_call);
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
        output::candidate_output(candidate, working, &ranking_provenance_context, reasons)
    });
    let recommendation = ranked_results.next();
    let alternatives: Vec<_> = ranked_results.take(ALTERNATIVE_LIMIT).collect();
    let summary = NextSummary::from_graph(&ready, candidate_count, &graph);
    NextAnalysis::from_search(NextResult {
        input_hash: effective_input_hash(working, scope),
        pending: working.is_pending(),
        pending_operation_ids: working.operation_ids(),
        horizon,
        state_budget: STATE_BUDGET,
        mode,
        pagerank_available: pagerank.is_some(),
        recommendation,
        alternatives,
        comparison_to_runner_up: comparison,
        close_call,
        search_complete,
        truncated_by,
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

fn priority(working: &WorkingGraph<'_>, issue: &Issue) -> PriorityComparison {
    working.priority(issue).comparison()
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
