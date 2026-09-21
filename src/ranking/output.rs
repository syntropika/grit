use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ALTERNATIVE_LIMIT, EvaluatedCandidate,
    decision::{PriorityProfile, RankingMode, StepPriority},
    explanation::{ComparisonEvidence, ModeReason},
    pagerank,
    search::SearchRestriction,
};
use crate::{
    model::{Issue, strip_operation_markers},
    operational::{OperationalGraph, ReadyAnalysis},
    priority::PriorityState,
    working_graph::{PendingProvenance, WorkingGraph},
};

#[derive(Clone, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct PresentedModeReason {
    #[serde(flatten)]
    provenance: PendingProvenance,
    reason: ModeReason,
    message: String,
}

impl PresentedModeReason {
    fn new(reason: ModeReason, provenance: PendingProvenance) -> Self {
        let message = reason.human_message().to_owned();
        Self {
            reason,
            message,
            provenance,
        }
    }
}

impl<'de> Deserialize<'de> for PresentedModeReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(flatten)]
            provenance: PendingProvenance,
            reason: ModeReason,
            message: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.message != wire.reason.human_message() {
            return Err(serde::de::Error::custom("invalid mode-reason message"));
        }
        Ok(Self {
            provenance: wire.provenance,
            reason: wire.reason,
            message: wire.message,
        })
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[schemars(deny_unknown_fields)]
pub(crate) struct NextAnalysis {
    #[serde(flatten)]
    decision: DecisionCore<NextParameters>,
    alternatives: Vec<CandidateResult>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[schemars(deny_unknown_fields)]
pub(crate) struct PlanDecision {
    #[serde(flatten)]
    decision: DecisionCore<PlanParameters>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[schemars(deny_unknown_fields)]
struct DecisionCore<P> {
    parameters: P,
    #[serde(flatten)]
    result: DecisionResult,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct DecisionResult {
    input_hash: String,
    pending: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_operation_ids: Vec<String>,
    mode: RankingMode,
    metrics: MetricStates,
    recommendation: Option<CandidateResult>,
    comparison_to_runner_up: Option<ComparisonEvidence>,
    close_call: bool,
    search_complete: bool,
    truncated_by: Vec<SearchRestriction>,
    global_optimum_claimed: bool,
    runner_up_scope: RunnerUpScope,
    summary: NextSummary,
    work: WorkCounts,
}

impl<P> DecisionCore<P> {
    fn map_parameters<Q>(self, map: impl FnOnce(P) -> Q) -> DecisionCore<Q> {
        DecisionCore {
            parameters: map(self.parameters),
            result: self.result,
        }
    }

    fn summary(&self) -> &NextSummary {
        &self.result.summary
    }

    fn truncation_warning(&self) -> Option<String> {
        truncation_warning(self.result.search_complete, &self.result.truncated_by)
    }

    fn input_hash(&self) -> &str {
        &self.result.input_hash
    }

    fn human_recommendation_summary(&self) -> Option<String> {
        self.result.recommendation.as_ref().map(|recommendation| {
            let reason = self
                .result
                .comparison_to_runner_up
                .as_ref()
                .map(ComparisonEvidence::human_message)
                .or_else(|| {
                    recommendation
                        .reasons
                        .iter()
                        .find(|reason| reason.reason.is_only_candidate())
                        .map(|reason| reason.message.as_str())
                })
                .unwrap_or("it is the deterministic best executable first step");
            recommendation.human_summary(reason)
        })
    }
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
        let search_complete = result.search_complete;
        Self {
            decision: DecisionCore {
                parameters: NextParameters {
                    common: CommonParameters {
                        horizon: result.horizon,
                        state_budget: result.state_budget,
                        pagerank: PageRankParameters {
                            damping: pagerank::DAMPING,
                            iterations: pagerank::ITERATIONS,
                            bucket_scale: pagerank::BUCKET_SCALE as u64,
                        },
                    },
                    alternative_limit: ALTERNATIVE_LIMIT,
                },
                result: DecisionResult {
                    input_hash: result.input_hash,
                    pending: result.pending,
                    pending_operation_ids: result.pending_operation_ids,
                    mode: result.mode,
                    metrics: MetricStates {
                        unlock_profile: MetricState {
                            state: MetricAvailability::Available,
                        },
                        pagerank,
                    },
                    recommendation: result.recommendation,
                    comparison_to_runner_up: result.comparison_to_runner_up,
                    close_call: result.close_call,
                    search_complete,
                    truncated_by: result.truncated_by,
                    global_optimum_claimed: search_complete,
                    runner_up_scope: if search_complete {
                        RunnerUpScope::Global
                    } else {
                        RunnerUpScope::Explored
                    },
                    summary: result.summary,
                    work: WorkCounts {
                        materialized_successors: result.work.materialized_successors,
                        probed_successors: result.work.probed_successors,
                    },
                },
            },
            alternatives: result.alternatives,
        }
    }

    pub(crate) fn summary(&self) -> &NextSummary {
        self.decision.summary()
    }

    pub(crate) fn truncation_warning(&self) -> Option<String> {
        self.decision.truncation_warning()
    }

    pub(crate) fn input_hash(&self) -> &str {
        self.decision.input_hash()
    }

    pub(crate) fn human_recommendation_summary(&self) -> Option<String> {
        self.decision.human_recommendation_summary()
    }

    pub(crate) fn into_plan_decision(self) -> PlanDecision {
        PlanDecision {
            decision: self.decision.map_parameters(|parameters| PlanParameters {
                common: parameters.common,
            }),
        }
    }
}

impl PlanDecision {
    pub(crate) fn summary(&self) -> &NextSummary {
        self.decision.summary()
    }

    pub(crate) fn truncation_warning(&self) -> Option<String> {
        self.decision.truncation_warning()
    }

    pub(crate) fn input_hash(&self) -> &str {
        self.decision.input_hash()
    }

    pub(crate) fn human_recommendation_summary(&self) -> Option<String> {
        self.decision.human_recommendation_summary()
    }
}

fn truncation_warning(search_complete: bool, truncated_by: &[SearchRestriction]) -> Option<String> {
    (!search_complete).then(|| {
        format!(
            "next/v1 search was restricted by {}; this is the best explored recommendation and no global optimum is claimed",
            truncated_by
                .iter()
                .map(|restriction| restriction.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

pub(super) struct NextResult {
    pub(super) input_hash: String,
    pub(super) pending: bool,
    pub(super) pending_operation_ids: Vec<String>,
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
    pub(super) work: super::search::SearchWork,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkCounts {
    materialized_successors: usize,
    probed_successors: usize,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[schemars(deny_unknown_fields)]
struct NextParameters {
    #[serde(flatten)]
    common: CommonParameters,
    alternative_limit: usize,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[schemars(deny_unknown_fields)]
struct PlanParameters {
    #[serde(flatten)]
    common: CommonParameters,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct CommonParameters {
    horizon: u8,
    state_budget: usize,
    pagerank: PageRankParameters,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct PageRankParameters {
    damping: f64,
    iterations: usize,
    bucket_scale: u64,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct MetricStates {
    unlock_profile: MetricState,
    pagerank: PageRankMetricState,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct MetricState {
    state: MetricAvailability,
}

#[derive(Clone, Copy, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
enum MetricAvailability {
    Available,
    Omitted,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct PageRankMetricState {
    state: MetricAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    damping: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iterations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bucket_scale: Option<u64>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateResult {
    #[serde(flatten)]
    provenance: PendingProvenance,
    first_issue: IssueReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    critical_distance: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pagerank_bucket: Option<u64>,
    rollout: Rollout,
    outcome: Outcome,
    reasons: Vec<PresentedModeReason>,
}

impl CandidateResult {
    fn human_summary(&self, reason: &str) -> String {
        let pending = if self.provenance.is_pending() {
            " [pending]"
        } else {
            ""
        };
        format!(
            "#{} {} [{}]{}: {} (unlocks {})",
            self.first_issue.number,
            self.first_issue.title,
            self.first_issue.priority.display_name(),
            pending,
            reason,
            self.outcome.unlock_profile.count
        )
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IssueReference {
    #[serde(flatten)]
    provenance: PendingProvenance,
    key: String,
    number: u64,
    url: String,
    title: String,
    priority: PriorityState,
    availability: Availability,
}

#[derive(Clone, Copy, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
enum Availability {
    Available,
    Assigned,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct Rollout {
    steps: Vec<RolloutStep>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct RolloutStep {
    position: u8,
    mode: StepMode,
    issue: IssueReference,
}

#[derive(Clone, Copy, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepMode {
    P0Ready,
    P0Route,
    Normal,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    unlock_profile: UnlockProfile,
    unlocks: Vec<Unlock>,
    unlock_availability: UnlockAvailability,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct UnlockProfile {
    count: usize,
    priority_profile: PriorityProfile,
    curve: Vec<usize>,
    p0_curve: Vec<usize>,
    step_priorities: Vec<StepPriority>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct Unlock {
    issue: IssueReference,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct UnlockAvailability {
    available: usize,
    assigned: usize,
}

#[derive(Clone, Copy, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunnerUpScope {
    Global,
    Explored,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
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
    working: &WorkingGraph<'_>,
    ranking_provenance_context: &[u64],
    reasons: Vec<ModeReason>,
) -> CandidateResult {
    let (candidate, critical_route) = candidate.into_parts();
    let first_issue = issue_reference(working, candidate.issue);
    let provenance = working.provenance_for_issues(
        candidate
            .steps
            .iter()
            .map(|step| step.issue.number)
            .chain(candidate.unlocks.iter().map(|issue| issue.number))
            .chain(ranking_provenance_context.iter().copied()),
    );
    let available_unlocks = candidate
        .unlocks
        .iter()
        .filter(|issue| issue.assignees.is_empty())
        .count();
    let unlocks = candidate
        .unlocks
        .iter()
        .map(|issue| Unlock {
            issue: issue_reference(working, issue),
        })
        .collect();
    CandidateResult {
        provenance: provenance.clone(),
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
                    issue: issue_reference(working, step.issue),
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
        reasons: reasons
            .into_iter()
            .map(|reason| PresentedModeReason::new(reason, provenance.clone()))
            .collect(),
    }
}

pub(super) fn issue_reference(working: &WorkingGraph<'_>, issue: &Issue) -> IssueReference {
    IssueReference {
        provenance: working.provenance_for_issue(issue.number),
        key: format!("{}#{}", working.replica().repository, issue.number),
        number: issue.number,
        url: issue.url.clone(),
        title: strip_operation_markers(&issue.title),
        priority: working.priority(issue),
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
