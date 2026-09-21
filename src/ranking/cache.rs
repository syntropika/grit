use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    CandidateData, CriticalRouteOutcome, EvaluatedCandidate, EvaluatedStep, StepSelection,
    decision::{PriorityProfile, RankingMode, StepPriority},
    pagerank::PageRank,
    search::{SearchRestriction, SearchResult, SearchWork, frontier::frontier},
};
use crate::{
    atomic_file,
    operational::{ExecutionScope, OperationalGraph},
    priority::PriorityComparison,
    working_graph::WorkingGraph,
};

const CACHE_SCHEMA_VERSION: &str = "hyfa.ranking-cache/v1";
const CACHE_FILE_NAME: &str = "ranking-next-v1-cache.json";

#[derive(Default)]
pub(crate) struct RankingCache {
    path: Option<PathBuf>,
    memory: Option<CacheDocument>,
}

impl RankingCache {
    pub(crate) fn at(repository_directory: &Path) -> Self {
        Self {
            path: Some(repository_directory.join(CACHE_FILE_NAME)),
            memory: None,
        }
    }

    pub(super) fn lookup<'issues>(
        &mut self,
        key: &str,
        working: &WorkingGraph<'_>,
        graph: &OperationalGraph<'issues>,
        scope: ExecutionScope<'_>,
        horizon: u8,
    ) -> Option<(Option<PageRank>, SearchResult<'issues>)> {
        if self.memory.is_none() {
            self.memory = self
                .path
                .as_ref()
                .and_then(|path| fs::read(path).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        }
        let document = self.memory.as_ref()?;
        if document.schema_version != CACHE_SCHEMA_VERSION || document.key != key {
            return None;
        }
        if document.payload_hash != payload_hash(&document.payload)? {
            return None;
        }
        Some((
            document.payload.pagerank.clone(),
            document.payload.search.hydrate(
                working,
                graph,
                scope,
                horizon,
                document.payload.pagerank.as_ref(),
            )?,
        ))
    }

    pub(super) fn publish(
        &mut self,
        key: String,
        pagerank: Option<&PageRank>,
        search: &SearchResult<'_>,
    ) -> bool {
        let payload = CachePayload {
            pagerank: pagerank.cloned(),
            search: CachedSearchResult::capture(search),
        };
        let Some(payload_hash) = payload_hash(&payload) else {
            return false;
        };
        let document = CacheDocument {
            schema_version: CACHE_SCHEMA_VERSION.to_owned(),
            key,
            payload_hash,
            payload,
        };
        let published = self.path.as_ref().is_none_or(|path| {
            let Some(parent) = path.parent() else {
                return false;
            };
            if fs::create_dir_all(parent).is_err() {
                return false;
            }
            let Ok(mut bytes) = serde_json::to_vec(&document) else {
                return false;
            };
            bytes.push(b'\n');
            atomic_file::publish(path, "ranking-cache", &bytes).is_ok()
        });
        if published {
            self.memory = Some(document);
        }
        published
    }
}

#[derive(Deserialize, Serialize)]
struct CacheDocument {
    schema_version: String,
    key: String,
    payload_hash: String,
    payload: CachePayload,
}

#[derive(Deserialize, Serialize)]
struct CachePayload {
    pagerank: Option<PageRank>,
    search: CachedSearchResult,
}

fn payload_hash(payload: &CachePayload) -> Option<String> {
    let canonical = serde_json::to_vec(payload).ok()?;
    Some(hex::encode(Sha256::digest(canonical)))
}

#[derive(Deserialize, Serialize)]
struct CachedSearchResult {
    mode: RankingMode,
    candidate_count: usize,
    candidates: Vec<CachedCandidate>,
    truncated_by: Vec<SearchRestriction>,
    work: SearchWork,
    provenance_numbers: std::collections::BTreeSet<u64>,
}

impl CachedSearchResult {
    fn capture(result: &SearchResult<'_>) -> Self {
        Self {
            mode: result.mode,
            candidate_count: result.candidate_count,
            candidates: result
                .candidates
                .iter()
                .map(CachedCandidate::capture)
                .collect(),
            truncated_by: result.truncated_by.clone(),
            work: result.work,
            provenance_numbers: result.provenance_numbers.clone(),
        }
    }

    fn hydrate<'issues>(
        &self,
        working: &WorkingGraph<'_>,
        graph: &OperationalGraph<'issues>,
        scope: ExecutionScope<'_>,
        horizon: u8,
        pagerank: Option<&PageRank>,
    ) -> Option<SearchResult<'issues>> {
        let p0_targets = graph
            .open_numbers()
            .iter()
            .copied()
            .filter(|number| {
                graph
                    .issue(*number)
                    .is_some_and(|issue| super::priority(working, issue) == PriorityComparison::P0)
            })
            .collect();
        let frontier = CachedFrontier {
            mode: self.mode,
            p0_targets,
        };
        let candidates = self
            .candidates
            .iter()
            .map(|candidate| candidate.hydrate(working, graph, scope, horizon, pagerank, &frontier))
            .collect::<Option<Vec<_>>>()?;
        Some(SearchResult {
            mode: self.mode,
            candidate_count: self.candidate_count,
            candidates,
            truncated_by: self.truncated_by.clone(),
            work: self.work,
            provenance_numbers: self.provenance_numbers.clone(),
        })
    }
}

struct CachedFrontier {
    mode: RankingMode,
    p0_targets: std::collections::BTreeSet<u64>,
}

#[derive(Deserialize, Serialize)]
struct CachedCandidate {
    kind: CachedCandidateKind,
    first_issue: u64,
    steps: Vec<CachedStep>,
    unlocks: Vec<u64>,
    priority_profile: PriorityProfile,
    unlock_curve: Vec<usize>,
    p0_curve: Vec<usize>,
    step_priorities: Vec<StepPriority>,
    pagerank_bucket: Option<u64>,
}

impl CachedCandidate {
    fn capture(candidate: &EvaluatedCandidate<'_>) -> Self {
        let data = candidate.data();
        let kind = match candidate {
            EvaluatedCandidate::Normal(_) => CachedCandidateKind::Normal,
            EvaluatedCandidate::P0Ready(_) => CachedCandidateKind::P0Ready,
            EvaluatedCandidate::CriticalRoute { route, .. } => CachedCandidateKind::CriticalRoute {
                feasible_distance: route.feasible_distance.get(),
                realized_distance: route.realized_distance.map(NonZeroUsize::get),
                qualifying_p0_count: route.qualifying_p0_count.get(),
            },
        };
        Self {
            kind,
            first_issue: data.issue.number,
            steps: data.steps.iter().map(CachedStep::capture).collect(),
            unlocks: data.unlocks.iter().map(|issue| issue.number).collect(),
            priority_profile: data.priority_profile,
            unlock_curve: data.unlock_curve.clone(),
            p0_curve: data.p0_curve.clone(),
            step_priorities: data.step_priorities.clone(),
            pagerank_bucket: data.pagerank_bucket,
        }
    }

    fn hydrate<'issues>(
        &self,
        working: &WorkingGraph<'_>,
        graph: &OperationalGraph<'issues>,
        scope: ExecutionScope<'_>,
        horizon: u8,
        pagerank: Option<&PageRank>,
        frontier: &CachedFrontier,
    ) -> Option<EvaluatedCandidate<'issues>> {
        let issue = graph.issue(self.first_issue)?;
        let mode = frontier.mode;
        let steps = self.validate_rollout(
            working,
            graph,
            scope,
            horizon,
            pagerank,
            &frontier.p0_targets,
        )?;
        if steps.first().map(|step| step.issue.number) != Some(self.first_issue) {
            return None;
        }
        if steps.first().map(|step| step.selection.mode()) != Some(mode) {
            return None;
        }
        let unlocks = self
            .unlocks
            .iter()
            .map(|number| graph.issue(*number))
            .collect::<Option<Vec<_>>>()?;
        let data = CandidateData {
            issue,
            steps,
            unlocks,
            priority_profile: self.priority_profile,
            unlock_curve: self.unlock_curve.clone(),
            p0_curve: self.p0_curve.clone(),
            step_priorities: self.step_priorities.clone(),
            pagerank_bucket: self.pagerank_bucket,
        };
        match (&self.kind, mode) {
            (CachedCandidateKind::Normal, RankingMode::Normal) => {
                Some(EvaluatedCandidate::Normal(data))
            }
            (CachedCandidateKind::P0Ready, RankingMode::P0Ready) => {
                Some(EvaluatedCandidate::P0Ready(data))
            }
            (
                CachedCandidateKind::CriticalRoute {
                    feasible_distance,
                    realized_distance,
                    qualifying_p0_count,
                },
                RankingMode::P0Route,
            ) => Some(EvaluatedCandidate::CriticalRoute {
                candidate: data,
                route: CriticalRouteOutcome {
                    feasible_distance: NonZeroUsize::new(*feasible_distance)?,
                    realized_distance: realized_distance.and_then(NonZeroUsize::new),
                    qualifying_p0_count: NonZeroUsize::new(*qualifying_p0_count)?,
                },
            }),
            _ => None,
        }
    }

    fn validate_rollout<'issues>(
        &self,
        working: &WorkingGraph<'_>,
        graph: &OperationalGraph<'issues>,
        scope: ExecutionScope<'_>,
        horizon: u8,
        pagerank: Option<&PageRank>,
        p0_targets: &std::collections::BTreeSet<u64>,
    ) -> Option<Vec<EvaluatedStep<'issues>>> {
        if self.steps.is_empty() || self.steps.len() > horizon as usize {
            return None;
        }
        let mut rollout = graph.rollout_state(scope);
        let mut steps = Vec::with_capacity(self.steps.len());
        let mut unlocks = std::collections::BTreeSet::new();
        let mut unlock_curve = Vec::with_capacity(horizon as usize);
        let mut p0_curve = Vec::with_capacity(horizon as usize);
        for (index, cached_step) in self.steps.iter().enumerate() {
            let remaining_steps = horizon as usize - index;
            let expected = frontier(&rollout, remaining_steps, p0_targets)
                .steps
                .into_iter()
                .find(|step| step.issue.number == cached_step.issue)?;
            if cached_step.selection != CachedStepSelection::capture(expected.selection) {
                return None;
            }
            let step = expected;
            let completion = rollout.complete(step.issue.number)?;
            unlocks.extend(completion.newly_ready().iter().copied());
            unlock_curve.push(unlocks.len());
            p0_curve.push(
                unlocks
                    .iter()
                    .filter_map(|number| graph.issue(*number))
                    .filter(|issue| super::priority(working, issue) == PriorityComparison::P0)
                    .count(),
            );
            steps.push(step);
        }
        unlock_curve.resize(
            horizon as usize,
            unlock_curve.last().copied().unwrap_or_default(),
        );
        p0_curve.resize(
            horizon as usize,
            p0_curve.last().copied().unwrap_or_default(),
        );
        let mut priority_profile = PriorityProfile::default();
        for number in &unlocks {
            let issue = graph.issue(*number)?;
            let issue_priority = super::priority(working, issue);
            if issue_priority != PriorityComparison::P0 {
                priority_profile.record(issue_priority);
            }
        }
        let mut step_priorities = steps
            .iter()
            .map(|step| StepPriority::from(super::priority(working, step.issue)))
            .collect::<Vec<_>>();
        step_priorities.resize(horizon as usize, StepPriority::NoStep);
        if self.unlocks != unlocks.into_iter().collect::<Vec<_>>()
            || self.priority_profile != priority_profile
            || self.unlock_curve != unlock_curve
            || self.p0_curve != p0_curve
            || self.step_priorities != step_priorities
            || self.pagerank_bucket
                != pagerank.and_then(|pagerank| pagerank.bucket(self.first_issue))
        {
            return None;
        }
        match (&self.kind, steps.first()?.selection) {
            (CachedCandidateKind::Normal, StepSelection::Normal)
            | (CachedCandidateKind::P0Ready, StepSelection::P0Ready) => Some(steps),
            (
                CachedCandidateKind::CriticalRoute {
                    feasible_distance,
                    realized_distance,
                    qualifying_p0_count,
                },
                StepSelection::P0Route {
                    feasible_distance: step_distance,
                    qualifying_p0_count: step_p0_count,
                },
            ) if *feasible_distance == step_distance.get()
                && *qualifying_p0_count == step_p0_count.get()
                && *realized_distance
                    == p0_curve
                        .iter()
                        .position(|count| *count > 0)
                        .map(|index| index + 1) =>
            {
                Some(steps)
            }
            _ => None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CachedCandidateKind {
    Normal,
    P0Ready,
    CriticalRoute {
        feasible_distance: usize,
        realized_distance: Option<usize>,
        qualifying_p0_count: usize,
    },
}

#[derive(Deserialize, Serialize)]
struct CachedStep {
    issue: u64,
    selection: CachedStepSelection,
}

impl CachedStep {
    fn capture(step: &EvaluatedStep<'_>) -> Self {
        Self {
            issue: step.issue.number,
            selection: CachedStepSelection::capture(step.selection),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CachedStepSelection {
    P0Ready,
    P0Route {
        feasible_distance: usize,
        qualifying_p0_count: usize,
    },
    Normal,
}

impl CachedStepSelection {
    fn capture(selection: StepSelection) -> Self {
        match selection {
            StepSelection::P0Ready => CachedStepSelection::P0Ready,
            StepSelection::P0Route {
                feasible_distance,
                qualifying_p0_count,
            } => CachedStepSelection::P0Route {
                feasible_distance: feasible_distance.get(),
                qualifying_p0_count: qualifying_p0_count.get(),
            },
            StepSelection::Normal => CachedStepSelection::Normal,
        }
    }
}
