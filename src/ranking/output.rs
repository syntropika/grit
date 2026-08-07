use serde::Serialize;

use super::{
    ALTERNATIVE_LIMIT, EvaluatedCandidate,
    decision::{PriorityProfile, RankingMode, StepPriority},
    explanation::{ComparisonEvidence, Reason},
    pagerank,
    search::SearchRestriction,
};
use crate::{
    model::Issue,
    operational::{OperationalGraph, ReadyAnalysis},
    priority::PriorityState,
};

#[derive(Serialize)]
pub(crate) struct NextAnalysis {
    input_hash: String,
    mode: RankingMode,
    parameters: Parameters,
    metrics: MetricStates,
    recommendation: Option<CandidateResult>,
    alternatives: Vec<CandidateResult>,
    comparison_to_runner_up: Option<ComparisonEvidence>,
    close_call: bool,
    search_complete: bool,
    truncated_by: Vec<SearchRestriction>,
    global_optimum_claimed: bool,
    runner_up_scope: RunnerUpScope,
    summary: NextSummary,
}

impl NextAnalysis {
    pub(super) fn from_search(result: NextResult) -> Self {
        let pagerank = if result.pagerank_available {
            PageRankMetricState {
                state: MetricAvailability::Available,
                damping: Some(pagerank::DAMPING),
                iterations: Some(pagerank::ITERATIONS),
                bucket_scale: Some(pagerank::BUCKET_SCALE as u64),
            }
        } else {
            PageRankMetricState {
                state: MetricAvailability::Omitted,
                damping: None,
                iterations: None,
                bucket_scale: None,
            }
        };
        Self {
            input_hash: result.input_hash,
            mode: result.mode,
            parameters: Parameters {
                horizon: result.horizon,
                alternative_limit: ALTERNATIVE_LIMIT,
                state_budget: result.state_budget,
                pagerank: PageRankParameters {
                    damping: pagerank::DAMPING,
                    iterations: pagerank::ITERATIONS,
                    bucket_scale: pagerank::BUCKET_SCALE as u64,
                },
            },
            metrics: MetricStates {
                unlock_profile: MetricState {
                    state: MetricAvailability::Available,
                },
                pagerank,
            },
            recommendation: result.recommendation,
            alternatives: result.alternatives,
            comparison_to_runner_up: result.comparison_to_runner_up,
            close_call: result.close_call,
            search_complete: result.search_complete,
            truncated_by: result.truncated_by,
            global_optimum_claimed: result.search_complete,
            runner_up_scope: if result.search_complete {
                RunnerUpScope::Global
            } else {
                RunnerUpScope::Explored
            },
            summary: result.summary,
        }
    }

    pub(crate) fn recommendation(&self) -> Option<&CandidateResult> {
        self.recommendation.as_ref()
    }

    pub(crate) fn summary(&self) -> &NextSummary {
        &self.summary
    }

    pub(crate) fn truncation_warning(&self) -> Option<String> {
        (!self.search_complete).then(|| {
            format!(
                "next/v1 search was restricted by {}; this is the best explored recommendation and no global optimum is claimed",
                self.truncated_by
                    .iter()
                    .map(|restriction| restriction.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }
}

pub(super) struct NextResult {
    pub(super) input_hash: String,
    pub(super) horizon: u8,
    pub(super) state_budget: usize,
    pub(super) mode: RankingMode,
    pub(super) pagerank_available: bool,
    pub(super) recommendation: Option<CandidateResult>,
    pub(super) alternatives: Vec<CandidateResult>,
    pub(super) comparison_to_runner_up: Option<ComparisonEvidence>,
    pub(super) close_call: bool,
    pub(super) search_complete: bool,
    pub(super) truncated_by: Vec<SearchRestriction>,
    pub(super) summary: NextSummary,
}

#[derive(Serialize)]
struct Parameters {
    horizon: u8,
    alternative_limit: usize,
    state_budget: usize,
    pagerank: PageRankParameters,
}

#[derive(Serialize)]
struct PageRankParameters {
    damping: f64,
    iterations: usize,
    bucket_scale: u64,
}

#[derive(Serialize)]
struct MetricStates {
    unlock_profile: MetricState,
    pagerank: PageRankMetricState,
}

#[derive(Serialize)]
struct MetricState {
    state: MetricAvailability,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum MetricAvailability {
    Available,
    Omitted,
}

#[derive(Serialize)]
struct PageRankMetricState {
    state: MetricAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    damping: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iterations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bucket_scale: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct CandidateResult {
    first_issue: IssueReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    critical_distance: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pagerank_bucket: Option<u64>,
    rollout: Rollout,
    outcome: Outcome,
    reasons: Vec<Reason>,
}

impl CandidateResult {
    pub(crate) fn human_summary(&self) -> String {
        let reason = self
            .reasons
            .last()
            .map(Reason::human_message)
            .unwrap_or("it is the deterministic best executable first step");
        format!(
            "#{} {} [{}]: {} (unlocks {})",
            self.first_issue.number,
            self.first_issue.title,
            self.first_issue.priority.display_name(),
            reason,
            self.outcome.unlock_profile.count
        )
    }
}

#[derive(Clone, Serialize)]
pub(super) struct IssueReference {
    key: String,
    number: u64,
    url: String,
    title: String,
    priority: PriorityState,
    availability: Availability,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Availability {
    Available,
    Assigned,
}

#[derive(Serialize)]
struct Rollout {
    steps: Vec<RolloutStep>,
}

#[derive(Serialize)]
struct RolloutStep {
    position: u8,
    mode: StepMode,
    issue: IssueReference,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepMode {
    P0Ready,
    P0Route,
    Normal,
}

#[derive(Serialize)]
struct Outcome {
    unlock_profile: UnlockProfile,
    unlocks: Vec<Unlock>,
    unlock_availability: UnlockAvailability,
}

#[derive(Serialize)]
struct UnlockProfile {
    count: usize,
    priority_profile: PriorityProfile,
    curve: Vec<usize>,
    p0_curve: Vec<usize>,
    step_priorities: Vec<StepPriority>,
}

#[derive(Serialize)]
struct Unlock {
    issue: IssueReference,
}

#[derive(Serialize)]
struct UnlockAvailability {
    available: usize,
    assigned: usize,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunnerUpScope {
    Global,
    Explored,
}

#[derive(Serialize)]
pub(crate) struct NextSummary {
    operational_issue_count: usize,
    ready_count: usize,
    executable_count: usize,
    candidate_count: usize,
    blocked_count: usize,
    assigned_ready_count: usize,
    cyclic_issue_count: usize,
    unknown_blocker_count: usize,
}

impl NextSummary {
    pub(super) fn from_graph(
        ready: &ReadyAnalysis<'_>,
        candidate_count: usize,
        graph: &OperationalGraph<'_>,
    ) -> Self {
        Self {
            operational_issue_count: ready.operational_issue_count,
            ready_count: ready.ready_count,
            executable_count: ready.executable.len(),
            candidate_count,
            blocked_count: ready.blocked_count,
            assigned_ready_count: ready.assigned_ready_count,
            cyclic_issue_count: graph.cyclic_numbers().len(),
            unknown_blocker_count: graph.unknown_blocker_count(),
        }
    }

    pub(crate) fn human_empty_summary(&self) -> String {
        format!(
            "No executable candidate: {} blocked, {} assigned Ready, {} cyclic, {} with an unknown blocker",
            self.blocked_count,
            self.assigned_ready_count,
            self.cyclic_issue_count,
            self.unknown_blocker_count
        )
    }
}

pub(super) fn candidate_output(
    candidate: EvaluatedCandidate<'_>,
    repository: &str,
    reasons: Vec<Reason>,
) -> CandidateResult {
    let (candidate, critical_route) = candidate.into_parts();
    let first_issue = issue_reference(repository, candidate.issue);
    let available_unlocks = candidate
        .unlocks
        .iter()
        .filter(|issue| issue.assignees.is_empty())
        .count();
    let unlocks = candidate
        .unlocks
        .iter()
        .map(|issue| Unlock {
            issue: issue_reference(repository, issue),
        })
        .collect();
    CandidateResult {
        first_issue: first_issue.clone(),
        critical_distance: critical_route.map(|route| route.distance().get()),
        pagerank_bucket: candidate.pagerank_bucket,
        rollout: Rollout {
            steps: candidate
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| RolloutStep {
                    position: (index + 1) as u8,
                    mode: step_mode(step.selection.mode()),
                    issue: issue_reference(repository, step.issue),
                })
                .collect(),
        },
        outcome: Outcome {
            unlock_profile: UnlockProfile {
                count: candidate.unlocks.len(),
                priority_profile: candidate.priority_profile,
                curve: candidate.unlock_curve,
                p0_curve: candidate.p0_curve,
                step_priorities: candidate.step_priorities,
            },
            unlocks,
            unlock_availability: UnlockAvailability {
                available: available_unlocks,
                assigned: candidate.unlocks.len() - available_unlocks,
            },
        },
        reasons,
    }
}

pub(super) fn issue_reference(repository: &str, issue: &Issue) -> IssueReference {
    IssueReference {
        key: format!("{repository}#{}", issue.number),
        number: issue.number,
        url: issue.url.clone(),
        title: issue.title.clone(),
        priority: PriorityState::from_issue_labels(&issue.labels),
        availability: if issue.assignees.is_empty() {
            Availability::Available
        } else {
            Availability::Assigned
        },
    }
}

fn step_mode(mode: RankingMode) -> StepMode {
    match mode {
        RankingMode::P0Ready => StepMode::P0Ready,
        RankingMode::P0Route => StepMode::P0Route,
        RankingMode::Normal | RankingMode::None => StepMode::Normal,
    }
}
