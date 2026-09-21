use serde::Serialize;
use serde_json::Value;

use super::{
    EvaluatedCandidate,
    decision::{DecisiveComparison, RankingMode},
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
    ShortestP0Route { critical_distance: u8 },
    SharedP0Prerequisite { unlocked_p0_count: usize },
}

impl ModeReason {
    fn human_message(&self) -> &'static str {
        match self {
            Self::OnlyExecutableCandidate { .. } => "it is the only executable candidate",
            Self::ReadyP0 { .. } => "it is an executable P0",
            Self::ShortestP0Route { .. } => "it makes a blocked P0 Ready in one step",
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
    let provenance = working.provenance_for_issues(
        winner
            .steps
            .iter()
            .map(|step| step.issue.number)
            .chain(winner.unlocks.iter().map(|issue| issue.number))
            .chain(runner_up.steps.iter().map(|step| step.issue.number))
            .chain(runner_up.unlocks.iter().map(|issue| issue.number))
            .chain(ranking_provenance_context.iter().copied()),
    );
    ComparisonEvidence {
        provenance,
        reason_code: descriptor.reason_code,
        component: descriptor.component,
        winner: issue_reference(working, winner.issue),
        runner_up: issue_reference(working, runner_up.issue),
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
    mode: RankingMode,
    executable_p0_count: usize,
    candidate: &EvaluatedCandidate<'_>,
) -> Vec<Reason> {
    let mut reasons = match mode {
        RankingMode::P0Ready => vec![Reason::Mode(ModeReason::ReadyP0 {
            executable_p0_count,
        })],
        RankingMode::P0Route => vec![Reason::Mode(ModeReason::ShortestP0Route {
            critical_distance: 1,
        })],
        RankingMode::Normal | RankingMode::None => Vec::new(),
    };
    let p0_unlock_count = candidate.p0_curve.last().copied().unwrap_or(0);
    if p0_unlock_count > 1 {
        reasons.push(Reason::Mode(ModeReason::SharedP0Prerequisite {
            unlocked_p0_count: p0_unlock_count,
        }));
    }
    reasons
}

pub(super) fn only_candidate_reason(candidate_count: usize) -> Reason {
    Reason::Mode(ModeReason::OnlyExecutableCandidate { candidate_count })
}
