use std::cmp::Ordering;

use serde::Serialize;

use super::EvaluatedCandidate;
use crate::priority::PriorityComparison;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RankingMode {
    None,
    P0Ready,
    P0Route,
    Normal,
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

#[derive(Clone, Copy)]
pub(super) enum DecisiveComparison {
    P0Curve {
        left: usize,
        right: usize,
    },
    UnlockCount {
        left: usize,
        right: usize,
    },
    UnlockPriorityProfile {
        left: PriorityProfile,
        right: PriorityProfile,
    },
    StepPriority {
        left: PriorityComparison,
        right: PriorityComparison,
    },
    PageRankBucket {
        left: u64,
        right: u64,
    },
    StableNodeKey {
        left: [u64; 2],
        right: [u64; 2],
    },
}

impl DecisiveComparison {
    pub(super) fn is_close_call(self) -> bool {
        matches!(
            self,
            Self::PageRankBucket { .. } | Self::StableNodeKey { .. }
        )
    }

    pub(super) fn reason_code(self) -> &'static str {
        match self {
            Self::P0Curve { .. } => "unlocks_more_p0",
            Self::UnlockCount { .. } => "unlocks_more_work",
            Self::UnlockPriorityProfile { .. } => "unlocks_higher_priority_work",
            Self::StepPriority { .. } => "declared_priority_tiebreak",
            Self::PageRankBucket { .. } => "pagerank_tiebreak",
            Self::StableNodeKey { .. } => "deterministic_tiebreak",
        }
    }

    pub(super) fn component(self) -> &'static str {
        match self {
            Self::P0Curve { .. } => "p0_curve",
            Self::UnlockCount { .. } => "unlock_count",
            Self::UnlockPriorityProfile { .. } => "unlock_priority_profile",
            Self::StepPriority { .. } => "step_priority",
            Self::PageRankBucket { .. } => "pagerank_bucket",
            Self::StableNodeKey { .. } => "stable_node_key",
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
    if mode.is_p0() && left.p0_unlock_count != right.p0_unlock_count {
        return CandidateComparison {
            ordering: left.p0_unlock_count.cmp(&right.p0_unlock_count),
            decisive: DecisiveComparison::P0Curve {
                left: left.p0_unlock_count,
                right: right.p0_unlock_count,
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
    if left.step_priority != right.step_priority {
        return CandidateComparison {
            ordering: priority_rank(left.step_priority).cmp(&priority_rank(right.step_priority)),
            decisive: DecisiveComparison::StepPriority {
                left: left.step_priority,
                right: right.step_priority,
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
        ordering: right.issue.number.cmp(&left.issue.number),
        decisive: DecisiveComparison::StableNodeKey {
            left: stable_key(left.issue.number),
            right: stable_key(right.issue.number),
        },
    }
}

fn priority_rank(priority: PriorityComparison) -> u8 {
    match priority {
        PriorityComparison::P0 => 5,
        PriorityComparison::P1 => 4,
        PriorityComparison::Neutral => 3,
        PriorityComparison::P3 => 2,
        PriorityComparison::P4 => 1,
    }
}

fn stable_key(issue_number: u64) -> [u64; 2] {
    [0, issue_number]
}
