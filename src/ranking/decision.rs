use std::cmp::Ordering;

use serde::Serialize;
use serde_json::{Value, json};

use super::EvaluatedCandidate;
use crate::{model::StableNodeKey, priority::PriorityComparison};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RankingMode {
    None,
    P0Ready,
    P0Route,
    Normal,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StepPriority {
    NoStep,
    P4,
    P3,
    Neutral,
    P1,
    P0,
}

impl From<PriorityComparison> for StepPriority {
    fn from(priority: PriorityComparison) -> Self {
        match priority {
            PriorityComparison::P0 => Self::P0,
            PriorityComparison::P1 => Self::P1,
            PriorityComparison::Neutral => Self::Neutral,
            PriorityComparison::P3 => Self::P3,
            PriorityComparison::P4 => Self::P4,
        }
    }
}

impl RankingMode {
    pub(super) fn is_p0(self) -> bool {
        matches!(self, Self::P0Ready | Self::P0Route)
    }
}

#[derive(Clone, Copy, Default, Eq, PartialEq, Serialize)]
pub(super) struct PriorityProfile {
    p1: usize,
    neutral: usize,
    p3: usize,
    p4: usize,
}

impl PriorityProfile {
    pub(super) fn record(&mut self, priority: PriorityComparison) {
        match priority {
            PriorityComparison::P0 => {}
            PriorityComparison::P1 => self.p1 += 1,
            PriorityComparison::Neutral => self.neutral += 1,
            PriorityComparison::P3 => self.p3 += 1,
            PriorityComparison::P4 => self.p4 += 1,
        }
    }

    fn as_array(self) -> [usize; 4] {
        [self.p1, self.neutral, self.p3, self.p4]
    }
}

#[derive(Clone)]
pub(super) enum DecisiveComparison {
    P0Curve {
        left: Vec<usize>,
        right: Vec<usize>,
    },
    UnlockCount {
        left: usize,
        right: usize,
    },
    UnlockPriorityProfile {
        left: PriorityProfile,
        right: PriorityProfile,
    },
    UnlockCurve {
        left: Vec<usize>,
        right: Vec<usize>,
    },
    StepPrioritySequence {
        left: Vec<StepPriority>,
        right: Vec<StepPriority>,
    },
    PageRankBucket {
        left: u64,
        right: u64,
    },
    StableNodeKey {
        left: StableNodeKey,
        right: StableNodeKey,
    },
}

impl DecisiveComparison {
    pub(super) fn is_close_call(&self) -> bool {
        self.descriptor().close_call
    }

    pub(super) fn descriptor(&self) -> ComparisonDescriptor {
        match self {
            Self::P0Curve { left, right } => ComparisonDescriptor::new(
                "unlocks_more_p0",
                "p0_curve",
                json!(left),
                json!(right),
                "it unlocks more P0 work",
                false,
            ),
            Self::UnlockCount { left, right } => ComparisonDescriptor::new(
                "unlocks_more_work",
                "unlock_count",
                json!(left),
                json!(right),
                "it unlocks more work",
                false,
            ),
            Self::UnlockPriorityProfile { left, right } => ComparisonDescriptor::new(
                "unlocks_higher_priority_work",
                "unlock_priority_profile",
                json!(left),
                json!(right),
                "it unlocks higher-priority work",
                false,
            ),
            Self::UnlockCurve { left, right } => ComparisonDescriptor::new(
                "unlocks_earlier",
                "unlock_curve",
                json!(left),
                json!(right),
                "it unlocks work earlier",
                false,
            ),
            Self::StepPrioritySequence { left, right } => ComparisonDescriptor::new(
                "declared_priority_tiebreak",
                "step_priority_sequence",
                json!(left),
                json!(right),
                "the rollout's step-Priority sequence breaks the tie",
                false,
            ),
            Self::PageRankBucket { left, right } => ComparisonDescriptor::new(
                "pagerank_tiebreak",
                "pagerank_bucket",
                json!(left),
                json!(right),
                "PageRank breaks an otherwise equal result",
                true,
            ),
            Self::StableNodeKey { left, right } => ComparisonDescriptor::new(
                "deterministic_tiebreak",
                "stable_node_key",
                json!(left),
                json!(right),
                "the Stable node key breaks a complete tie",
                true,
            ),
        }
    }
}

pub(super) struct ComparisonDescriptor {
    pub(super) reason_code: &'static str,
    pub(super) component: &'static str,
    pub(super) winner_value: Value,
    pub(super) runner_up_value: Value,
    pub(super) human_message: &'static str,
    close_call: bool,
}

impl ComparisonDescriptor {
    fn new(
        reason_code: &'static str,
        component: &'static str,
        winner_value: Value,
        runner_up_value: Value,
        human_message: &'static str,
        close_call: bool,
    ) -> Self {
        Self {
            reason_code,
            component,
            winner_value,
            runner_up_value,
            human_message,
            close_call,
        }
    }
}

pub(super) struct CandidateComparison {
    pub(super) ordering: Ordering,
    pub(super) decisive: DecisiveComparison,
}

pub(super) fn compare(
    left: &EvaluatedCandidate<'_>,
    right: &EvaluatedCandidate<'_>,
    mode: RankingMode,
) -> CandidateComparison {
    if mode.is_p0() && left.p0_curve != right.p0_curve {
        return CandidateComparison {
            ordering: left.p0_curve.cmp(&right.p0_curve),
            decisive: DecisiveComparison::P0Curve {
                left: left.p0_curve.clone(),
                right: right.p0_curve.clone(),
            },
        };
    }
    if left.unlocks.len() != right.unlocks.len() {
        return CandidateComparison {
            ordering: left.unlocks.len().cmp(&right.unlocks.len()),
            decisive: DecisiveComparison::UnlockCount {
                left: left.unlocks.len(),
                right: right.unlocks.len(),
            },
        };
    }
    if left.priority_profile != right.priority_profile {
        return CandidateComparison {
            ordering: left
                .priority_profile
                .as_array()
                .cmp(&right.priority_profile.as_array()),
            decisive: DecisiveComparison::UnlockPriorityProfile {
                left: left.priority_profile,
                right: right.priority_profile,
            },
        };
    }
    if left.unlock_curve != right.unlock_curve {
        return CandidateComparison {
            ordering: left.unlock_curve.cmp(&right.unlock_curve),
            decisive: DecisiveComparison::UnlockCurve {
                left: left.unlock_curve.clone(),
                right: right.unlock_curve.clone(),
            },
        };
    }
    if left.step_priorities != right.step_priorities {
        return CandidateComparison {
            ordering: left.step_priorities.cmp(&right.step_priorities),
            decisive: DecisiveComparison::StepPrioritySequence {
                left: left.step_priorities.clone(),
                right: right.step_priorities.clone(),
            },
        };
    }
    if left.pagerank_bucket != right.pagerank_bucket {
        return CandidateComparison {
            ordering: left.pagerank_bucket.cmp(&right.pagerank_bucket),
            decisive: DecisiveComparison::PageRankBucket {
                left: left.pagerank_bucket.unwrap_or_default(),
                right: right.pagerank_bucket.unwrap_or_default(),
            },
        };
    }
    CandidateComparison {
        ordering: right
            .issue
            .stable_node_key()
            .cmp(&left.issue.stable_node_key()),
        decisive: DecisiveComparison::StableNodeKey {
            left: left.issue.stable_node_key(),
            right: right.issue.stable_node_key(),
        },
    }
}
