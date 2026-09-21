use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use serde_json::json;
use sha2::{Digest, Sha256};

mod cache;
mod decision;
mod explanation;
mod output;
mod pagerank;
#[cfg(test)]
mod performance_tests;
mod search;

use crate::{
    model::Issue,
    operational::{ExecutionScope, PreparedRepository, ReadyAnalysis},
    priority::PriorityComparison,
    working_graph::WorkingGraph,
};
pub(crate) use cache::RankingCache;
use decision::{PriorityProfile, RankingMode, StepPriority};
pub(crate) use output::{NextAnalysis, PlanDecision};
use output::{NextResult, NextSummary};
use pagerank::PageRank;

pub(crate) const POLICY_VERSION: &str = "next/v1";
pub(crate) const OUTPUT_SCHEMA_VERSION: &str = "hyfa.next/v1";
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

pub(crate) struct AnalysisRun {
    pub(crate) analysis: NextAnalysis,
    pub(crate) profile: AnalysisProfile,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AnalysisProfile {
    pub(crate) graph_preparation: Duration,
    pub(crate) scc_detection: Duration,
    pub(crate) readiness: Duration,
    pub(crate) cache_lookup: Duration,
    pub(crate) pagerank: Duration,
    pub(crate) search: Duration,
    pub(crate) output_assembly: Duration,
    pub(crate) cache_publication: Duration,
    pub(crate) total: Duration,
    pub(crate) cache_hit: bool,
    pub(crate) cache_published: bool,
}

pub(crate) fn analyze_profiled(
    working: &WorkingGraph<'_>,
    scope: ExecutionScope<'_>,
    horizon: u8,
    cache: &mut RankingCache,
) -> AnalysisRun {
    let total_started = Instant::now();
    let (prepared, graph_timings) = PreparedRepository::prepare_profiled(working);
    let mut run = analyze_prepared(&prepared, scope, horizon, cache);
    run.profile.graph_preparation = graph_timings.graph_preparation;
    run.profile.scc_detection = graph_timings.scc_detection;
    run.profile.total = total_started.elapsed();
    run
}

pub(crate) fn analyze_prepared(
    prepared: &PreparedRepository<'_>,
    scope: ExecutionScope<'_>,
    horizon: u8,
    cache: &mut RankingCache,
) -> AnalysisRun {
    analyze_prepared_bundle(prepared, scope, horizon, cache).run
}

pub(crate) struct AnalysisBundle<'a> {
    pub(crate) run: AnalysisRun,
    pub(crate) ready: ReadyAnalysis<'a>,
    pub(crate) candidate_unlock_counts: BTreeMap<u64, usize>,
    pub(crate) pagerank_buckets: BTreeMap<u64, u64>,
}

pub(crate) fn analyze_prepared_bundle<'a>(
    prepared: &'a PreparedRepository<'a>,
    scope: ExecutionScope<'_>,
    horizon: u8,
    cache: &mut RankingCache,
) -> AnalysisBundle<'a> {
    let total_started = Instant::now();
    let working = prepared.working();
    let graph = prepared.graph();
    let readiness_started = Instant::now();
    let ready = graph.analyze_ready(scope);
    let readiness = readiness_started.elapsed();
    let input_hash = effective_input_hash(working, scope);
    let cache_key = ranking_cache_key(&input_hash, horizon);
    let cache_lookup_started = Instant::now();
    let cached = cache.lookup(&cache_key, working, graph, scope, horizon);
    let cache_lookup = cache_lookup_started.elapsed();
    let cache_hit = cached.is_some();
    let mut pagerank_duration = Duration::ZERO;
    let mut search_duration = Duration::ZERO;
    let mut cache_publication = Duration::ZERO;
    let mut cache_published = false;
    let (pagerank, search) = if let Some(cached) = cached {
        cached
    } else {
        let pagerank_started = Instant::now();
        let pagerank = PageRank::calculate(graph);
        pagerank_duration = pagerank_started.elapsed();
        let search_started = Instant::now();
        let search = search::evaluate(
            working,
            graph,
            scope,
            pagerank.as_ref(),
            horizon,
            STATE_BUDGET,
        );
        search_duration = search_started.elapsed();
        let cache_publication_started = Instant::now();
        cache_published = cache.publish(cache_key, pagerank.as_ref(), &search);
        cache_publication = cache_publication_started.elapsed();
        (pagerank, search)
    };
    let output_started = Instant::now();
    let pagerank_buckets = pagerank
        .as_ref()
        .map(|metric| metric.buckets().clone())
        .unwrap_or_default();
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
    let work = search.work;
    let ranking_provenance_context: Vec<_> = search.provenance_numbers.into_iter().collect();
    let evaluated = select_top_candidates(search.candidates, ALTERNATIVE_LIMIT + 1);
    let decisive = evaluated
        .first()
        .zip(evaluated.get(1))
        .map(|(winner, runner_up)| decision::compare(winner, runner_up).decisive);
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
    let executable_p0_count = ready
        .executable
        .iter()
        .filter(|issue| priority(working, issue) == PriorityComparison::P0)
        .count();

    let mut ranked_results = evaluated.into_iter().enumerate().map(|(index, candidate)| {
        let mut reasons = explanation::mode_reasons(executable_p0_count, &candidate);
        if index == 0 && candidate_count == 1 {
            reasons.push(explanation::only_candidate_reason(candidate_count));
        }
        output::candidate_output(candidate, working, &ranking_provenance_context, reasons)
    });
    let recommendation = ranked_results.next();
    let alternatives: Vec<_> = ranked_results.take(ALTERNATIVE_LIMIT).collect();
    let summary = NextSummary::from_graph(&ready, candidate_count, graph);
    let analysis = NextAnalysis::from_search(NextResult {
        input_hash,
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
        work,
    });
    let output_assembly = output_started.elapsed();
    AnalysisBundle {
        ready,
        candidate_unlock_counts,
        pagerank_buckets,
        run: AnalysisRun {
            analysis,
            profile: AnalysisProfile {
                graph_preparation: Duration::ZERO,
                scc_detection: Duration::ZERO,
                readiness,
                cache_lookup,
                pagerank: pagerank_duration,
                search: search_duration,
                output_assembly,
                cache_publication,
                total: total_started.elapsed(),
                cache_hit,
                cache_published,
            },
        },
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

fn priority(working: &WorkingGraph<'_>, issue: &Issue) -> PriorityComparison {
    working.priority(issue).comparison()
}

fn effective_input_hash(working: &WorkingGraph<'_>, scope: ExecutionScope<'_>) -> String {
    let (mode, assignee) = scope.hash_key();
    let input = json!({
        "schema_version": "hyfa.working-input/v1",
        "working_graph_hash": working.input_hash(),
        "execution_scope": {
            "mode": mode,
            "assignee": assignee,
        },
    });
    let canonical = serde_json::to_vec(&input).expect("effective input hash is serializable");
    hex::encode(Sha256::digest(canonical))
}

fn ranking_cache_key(input_hash: &str, horizon: u8) -> String {
    let input = json!({
        "schema_version": "hyfa.ranking-cache-key/v1",
        "effective_input_hash": input_hash,
        "policy_version": POLICY_VERSION,
        "parameters": {
            "horizon": horizon,
            "alternative_limit": ALTERNATIVE_LIMIT,
            "state_budget": STATE_BUDGET,
            "pagerank": {
                "damping": pagerank::DAMPING,
                "iterations": pagerank::ITERATIONS,
                "bucket_scale": pagerank::BUCKET_SCALE,
            }
        }
    });
    let canonical = serde_json::to_vec(&input).expect("ranking cache key is serializable");
    hex::encode(Sha256::digest(canonical))
}
