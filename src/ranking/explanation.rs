use serde::Serialize;
use serde_json::Value;

use super::{
    EvaluatedCandidate,
    decision::DecisiveComparison,
    output::{IssueReference, issue_reference},
};
use crate::working_graph::{PendingProvenance, WorkingGraph};

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Reason {
    Mode(ModeReason),
    Comparison(ComparisonReason),
}

impl Reason {
    pub(super) fn human_message(&self) -> &'static str {
        match self {
            Self::Mode(reason) => reason.human_message(),
            Self::Comparison(reason) => reason.human_message,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(super) enum ModeReason {
    OnlyExecutableCandidate { candidate_count: usize },
    ReadyP0 { executable_p0_count: usize },
    ShortestP0Route { critical_distance: usize },
    P0GateContinues { critical_step_count: usize },
    SharedP0Prerequisite { qualifying_p0_count: usize },
}

impl ModeReason {
    fn human_message(&self) -> &'static str {
        match self {
            Self::OnlyExecutableCandidate { .. } => "it is the only executable candidate",
            Self::ReadyP0 { .. } => "it is an executable P0",
            Self::ShortestP0Route { .. } => {
                "it follows the shortest feasible route to make blocked P0 work Ready"
            }
            Self::P0GateContinues { .. } => "P0 precedence is recalculated throughout the rollout",
            Self::SharedP0Prerequisite { .. } => "it is shared by multiple blocked P0 Issues",
        }
    }
}

#[derive(Serialize)]
pub(super) struct ComparisonReason {
    #[serde(rename = "code")]
    reason_code: &'static str,
    component: &'static str,
    winner_value: Value,
    runner_up_value: Value,
    #[serde(skip)]
    human_message: &'static str,
}

#[derive(Serialize)]
pub(super) struct ComparisonEvidence {
    #[serde(flatten)]
    provenance: PendingProvenance,
    reason_code: &'static str,
    component: &'static str,
    winner: IssueReference,
    runner_up: IssueReference,
    winner_value: Value,
    runner_up_value: Value,
}

pub(super) fn evidence(
    decision: &DecisiveComparison,
    winner: &EvaluatedCandidate<'_>,
    runner_up: &EvaluatedCandidate<'_>,
    working: &WorkingGraph<'_>,
    ranking_provenance_context: &[u64],
) -> ComparisonEvidence {
    let descriptor = decision.descriptor();
    let provenance = working.ranking_provenance_for_issues(
        winner
            .data()
            .steps
            .iter()
            .map(|step| step.issue.number)
            .chain(winner.data().unlocks.iter().map(|issue| issue.number))
            .chain(runner_up.data().steps.iter().map(|step| step.issue.number))
            .chain(runner_up.data().unlocks.iter().map(|issue| issue.number))
            .chain(ranking_provenance_context.iter().copied()),
    );
    ComparisonEvidence {
        provenance,
        reason_code: descriptor.reason_code,
        component: descriptor.component,
        winner: issue_reference(working, winner.data().issue),
        runner_up: issue_reference(working, runner_up.data().issue),
        winner_value: descriptor.winner_value,
        runner_up_value: descriptor.runner_up_value,
    }
}

pub(super) fn reason(decision: DecisiveComparison) -> Reason {
    let descriptor = decision.descriptor();
    Reason::Comparison(ComparisonReason {
        reason_code: descriptor.reason_code,
        component: descriptor.component,
        winner_value: descriptor.winner_value,
        runner_up_value: descriptor.runner_up_value,
        human_message: descriptor.human_message,
    })
}

pub(super) fn mode_reasons(
    executable_p0_count: usize,
    candidate: &EvaluatedCandidate<'_>,
) -> Vec<Reason> {
    let mut reasons = match candidate {
        EvaluatedCandidate::P0Ready(_) => vec![Reason::Mode(ModeReason::ReadyP0 {
            executable_p0_count,
        })],
        EvaluatedCandidate::CriticalRoute { route, .. } => {
            vec![Reason::Mode(ModeReason::ShortestP0Route {
                critical_distance: route.distance().get(),
            })]
        }
        EvaluatedCandidate::Normal(_) => Vec::new(),
    };
    if let EvaluatedCandidate::CriticalRoute { route, .. } = candidate
        && route.qualifying_p0_count.get() > 1
    {
        reasons.push(Reason::Mode(ModeReason::SharedP0Prerequisite {
            qualifying_p0_count: route.qualifying_p0_count.get(),
        }));
    }
    let critical_step_count = candidate
        .data()
        .steps
        .iter()
        .skip(1)
        .filter(|step| step.selection.mode().is_p0())
        .count();
    if critical_step_count > 0 {
        reasons.push(Reason::Mode(ModeReason::P0GateContinues {
            critical_step_count,
        }));
    }
    reasons
}

pub(super) fn only_candidate_reason(candidate_count: usize) -> Reason {
    Reason::Mode(ModeReason::OnlyExecutableCandidate { candidate_count })
}
