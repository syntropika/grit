use serde::Serialize;
use serde_json::{Value, json};

use super::{
    EvaluatedCandidate,
    decision::{DecisiveComparison, PriorityProfile, RankingMode},
    output::{IssueReference, issue_reference},
};
use crate::model::StableNodeKey;
use crate::priority::PriorityComparison;
use crate::working_graph::{PendingProvenance, WorkingGraph};

#[derive(Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(super) enum Reason {
    OnlyExecutableCandidate {
        candidate_count: usize,
    },
    ReadyP0 {
        executable_p0_count: usize,
    },
    ShortestP0Route {
        critical_distance: u8,
    },
    UnlocksMoreP0 {
        winner_count: usize,
        runner_up_count: usize,
    },
    SharedP0Prerequisite {
        unlocked_p0_count: usize,
    },
    UnlocksMoreWork {
        winner_count: usize,
        runner_up_count: usize,
    },
    UnlocksHigherPriorityWork {
        winner: PriorityProfile,
        runner_up: PriorityProfile,
    },
    DeclaredPriorityTiebreak {
        winner: PriorityComparison,
        runner_up: PriorityComparison,
    },
    PagerankTiebreak {
        winner_bucket: u64,
        runner_up_bucket: u64,
    },
    DeterministicTiebreak {
        winner_key: StableNodeKey,
        runner_up_key: StableNodeKey,
    },
}

impl Reason {
    pub(super) fn human_message(&self) -> &'static str {
        match self {
            Self::OnlyExecutableCandidate { .. } => "it is the only executable candidate",
            Self::ReadyP0 { .. } => "it is an executable P0",
            Self::ShortestP0Route { .. } => "it makes a blocked P0 Ready in one step",
            Self::UnlocksMoreP0 { .. } => "it unlocks more P0 work",
            Self::SharedP0Prerequisite { .. } => "it is shared by multiple blocked P0 Issues",
            Self::UnlocksMoreWork { .. } => "it unlocks more work",
            Self::UnlocksHigherPriorityWork { .. } => "it unlocks higher-priority work",
            Self::DeclaredPriorityTiebreak { .. } => "its Declared priority breaks the tie",
            Self::PagerankTiebreak { .. } => "PageRank breaks an otherwise equal result",
            Self::DeterministicTiebreak { .. } => "the Stable node key breaks a complete tie",
        }
    }
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
    decision: DecisiveComparison,
    winner: &EvaluatedCandidate<'_>,
    runner_up: &EvaluatedCandidate<'_>,
    working: &WorkingGraph<'_>,
    ranking_provenance_context: &[u64],
) -> ComparisonEvidence {
    let reason_code = decision.reason_code();
    let component = decision.component();
    let (winner_value, runner_up_value) = match decision {
        DecisiveComparison::P0Curve { left, right } => (json!([left]), json!([right])),
        DecisiveComparison::UnlockCount { left, right } => (json!(left), json!(right)),
        DecisiveComparison::UnlockPriorityProfile { left, right } => (json!(left), json!(right)),
        DecisiveComparison::StepPriority { left, right } => (json!(left), json!(right)),
        DecisiveComparison::PageRankBucket { left, right } => (json!(left), json!(right)),
        DecisiveComparison::StableNodeKey { left, right } => (json!(left), json!(right)),
    };
    let provenance = working.ranking_provenance_for_issues(
        std::iter::once(winner.issue.number)
            .chain(winner.unlocks.iter().map(|issue| issue.number))
            .chain(std::iter::once(runner_up.issue.number))
            .chain(runner_up.unlocks.iter().map(|issue| issue.number))
            .chain(ranking_provenance_context.iter().copied()),
    );
    ComparisonEvidence {
        provenance,
        reason_code,
        component,
        winner: issue_reference(working, winner.issue),
        runner_up: issue_reference(working, runner_up.issue),
        winner_value,
        runner_up_value,
    }
}

pub(super) fn reason(decision: DecisiveComparison) -> Reason {
    match decision {
        DecisiveComparison::P0Curve { left, right } => Reason::UnlocksMoreP0 {
            winner_count: left,
            runner_up_count: right,
        },
        DecisiveComparison::UnlockCount { left, right } => Reason::UnlocksMoreWork {
            winner_count: left,
            runner_up_count: right,
        },
        DecisiveComparison::UnlockPriorityProfile { left, right } => {
            Reason::UnlocksHigherPriorityWork {
                winner: left,
                runner_up: right,
            }
        }
        DecisiveComparison::StepPriority { left, right } => Reason::DeclaredPriorityTiebreak {
            winner: left,
            runner_up: right,
        },
        DecisiveComparison::PageRankBucket { left, right } => Reason::PagerankTiebreak {
            winner_bucket: left,
            runner_up_bucket: right,
        },
        DecisiveComparison::StableNodeKey { left, right } => Reason::DeterministicTiebreak {
            winner_key: left,
            runner_up_key: right,
        },
    }
}

pub(super) fn mode_reasons(
    mode: RankingMode,
    executable_p0_count: usize,
    candidate: &EvaluatedCandidate<'_>,
) -> Vec<Reason> {
    let mut reasons = match mode {
        RankingMode::P0Ready => vec![Reason::ReadyP0 {
            executable_p0_count,
        }],
        RankingMode::P0Route => vec![Reason::ShortestP0Route {
            critical_distance: 1,
        }],
        RankingMode::Normal | RankingMode::None => Vec::new(),
    };
    if candidate.p0_unlock_count > 1 {
        reasons.push(Reason::SharedP0Prerequisite {
            unlocked_p0_count: candidate.p0_unlock_count,
        });
    }
    reasons
}

pub(super) fn only_candidate_reason(candidate_count: usize) -> Reason {
    Reason::OnlyExecutableCandidate { candidate_count }
}
