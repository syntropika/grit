use std::{collections::BTreeMap, num::NonZeroUsize};

use serde_json::json;
use sha2::{Digest, Sha256};

mod decision;
mod explanation;
mod output;
mod pagerank;
mod search;

use crate::{
    model::{Issue, LocalReplica},
    operational::{ExecutionScope, PreparedRepository, ReadyAnalysis},
    priority::{PriorityComparison, PriorityState},
};
use decision::{PriorityProfile, RankingMode, StepPriority};
pub(crate) use output::{NextAnalysis, PlanDecision};
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
    selection: StepSelection,
}

#[derive(Clone, Copy)]
enum StepSelection {
    P0Ready,
    P0Route {
        feasible_distance: NonZeroUsize,
        qualifying_p0_count: NonZeroUsize,
    },
    Normal,
}

impl StepSelection {
    fn mode(self) -> RankingMode {
        match self {
            Self::P0Ready => RankingMode::P0Ready,
            Self::P0Route { .. } => RankingMode::P0Route,
            Self::Normal => RankingMode::Normal,
        }
    }
}

struct CandidateData<'a> {
    issue: &'a Issue,
    steps: Vec<EvaluatedStep<'a>>,
    unlocks: Vec<&'a Issue>,
    priority_profile: PriorityProfile,
    unlock_curve: Vec<usize>,
    p0_curve: Vec<usize>,
    step_priorities: Vec<StepPriority>,
    pagerank_bucket: Option<u64>,
}

struct CriticalRouteOutcome {
    feasible_distance: NonZeroUsize,
    realized_distance: Option<NonZeroUsize>,
    qualifying_p0_count: NonZeroUsize,
}

impl CriticalRouteOutcome {
    fn distance(&self) -> NonZeroUsize {
        self.realized_distance.unwrap_or(self.feasible_distance)
    }
}

enum EvaluatedCandidate<'a> {
    Normal(CandidateData<'a>),
    P0Ready(CandidateData<'a>),
    CriticalRoute {
        candidate: CandidateData<'a>,
        route: CriticalRouteOutcome,
    },
}

impl<'a> EvaluatedCandidate<'a> {
    fn data(&self) -> &CandidateData<'a> {
        match self {
            Self::Normal(candidate)
            | Self::P0Ready(candidate)
            | Self::CriticalRoute { candidate, .. } => candidate,
        }
    }

    fn into_parts(self) -> (CandidateData<'a>, Option<CriticalRouteOutcome>) {
        match self {
            Self::Normal(candidate) | Self::P0Ready(candidate) => (candidate, None),
            Self::CriticalRoute { candidate, route } => (candidate, Some(route)),
        }
    }
}

pub(crate) fn analyze(
    replica: &LocalReplica,
    scope: ExecutionScope<'_>,
    horizon: u8,
) -> NextAnalysis {
    let prepared = PreparedRepository::prepare(replica);
    analyze_prepared(&prepared, scope, horizon)
}

pub(crate) fn analyze_prepared(
    prepared: &PreparedRepository<'_>,
    scope: ExecutionScope<'_>,
    horizon: u8,
) -> NextAnalysis {
    analyze_prepared_bundle(prepared, scope, horizon).next
}

pub(crate) struct AnalysisBundle<'a> {
    pub(crate) next: NextAnalysis,
    pub(crate) ready: ReadyAnalysis<'a>,
    pub(crate) candidate_unlock_counts: BTreeMap<u64, usize>,
    pub(crate) pagerank_buckets: BTreeMap<u64, u64>,
}

pub(crate) fn analyze_prepared_bundle<'a>(
    prepared: &'a PreparedRepository<'a>,
    scope: ExecutionScope<'_>,
    horizon: u8,
) -> AnalysisBundle<'a> {
    let replica = prepared.replica();
    let graph = prepared.graph();
    let ready = graph.analyze_ready(scope);
    let pagerank = PageRank::calculate(graph);
    let pagerank_buckets = pagerank
        .as_ref()
        .map(|metric| metric.buckets().clone())
        .unwrap_or_default();
    let search = search::evaluate(graph, scope, pagerank.as_ref(), horizon, STATE_BUDGET);
    let mode = search.mode;
    let candidate_count = search.candidate_count;
    let search_complete = search.truncated_by.is_empty();
    let candidate_unlock_counts = search
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.data().issue.number,
                candidate.data().unlocks.len(),
            )
        })
        .collect();
    let truncated_by = search.truncated_by;
    let evaluated = select_top_candidates(search.candidates, ALTERNATIVE_LIMIT + 1);
    let decisive = evaluated
        .first()
        .zip(evaluated.get(1))
        .map(|(winner, runner_up)| decision::compare(winner, runner_up).decisive);
    let comparison = decisive
        .as_ref()
        .zip(evaluated.first().zip(evaluated.get(1)))
        .map(|(decision, (winner, runner_up))| {
            explanation::evidence(decision, winner, runner_up, &replica.repository)
        });
    let close_call = decisive
        .as_ref()
        .is_some_and(decision::DecisiveComparison::is_close_call);
    let executable_p0_count = ready
        .executable
        .iter()
        .filter(|issue| priority(issue) == PriorityComparison::P0)
        .count();

    let mut ranked_results = evaluated.into_iter().enumerate().map(|(index, candidate)| {
        let mut reasons = explanation::mode_reasons(executable_p0_count, &candidate);
        if index == 0 && candidate_count == 1 {
            reasons.push(explanation::only_candidate_reason(candidate_count));
        }
        output::candidate_output(candidate, &replica.repository, reasons)
    });
    let recommendation = ranked_results.next();
    let alternatives: Vec<_> = ranked_results.take(ALTERNATIVE_LIMIT).collect();
    let summary = NextSummary::from_graph(&ready, candidate_count, graph);
    let next = NextAnalysis::from_search(NextResult {
        input_hash: effective_input_hash(replica, scope),
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
    });
    AnalysisBundle {
        next,
        ready,
        candidate_unlock_counts,
        pagerank_buckets,
    }
}

fn select_top_candidates<'a>(
    candidates: impl IntoIterator<Item = EvaluatedCandidate<'a>>,
    limit: usize,
) -> Vec<EvaluatedCandidate<'a>> {
    let mut best = Vec::with_capacity(limit);
    for candidate in candidates {
        let position = best
            .iter()
            .position(|current| {
                decision::compare(&candidate, current).ordering == std::cmp::Ordering::Greater
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

fn priority(issue: &Issue) -> PriorityComparison {
    PriorityState::from_issue_labels(&issue.labels).comparison()
}

fn effective_input_hash(replica: &LocalReplica, scope: ExecutionScope<'_>) -> String {
    let (mode, assignee) = scope.hash_key();
    let input = json!({
        "schema_version": "grit.working-input/v1",
        "replica_snapshot_hash": replica.input_hash,
        "execution_scope": {
            "mode": mode,
            "assignee": assignee,
        }
    });
    let canonical = serde_json::to_vec(&input).expect("effective input hash is serializable");
    hex::encode(Sha256::digest(canonical))
}
