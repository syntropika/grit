use std::cmp::Ordering;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::EvaluatedCandidate;
use crate::{model::StableNodeKey, priority::PriorityComparison};

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub(super) enum RankingMode {
    #[default]
    None,
    P0Ready,
    P0Route,
    Normal,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
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

#[derive(Clone, Copy, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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

    pub(super) fn as_array(self) -> [usize; 4] {
        [self.p1, self.neutral, self.p3, self.p4]
    }
}

#[derive(Clone)]
pub(super) enum DecisiveComparison {
    CriticalDistance {
        left: usize,
        right: usize,
    },
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
            Self::CriticalDistance { left, right } => ComparisonDescriptor::new(
                "shortest_p0_route",
                "critical_distance",
                json!(left),
                json!(right),
                "it follows the shortest feasible route to P0 work",
                false,
            ),
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

pub(super) struct RankingKey<'a> {
    pub(super) mode: RankingMode,
    pub(super) critical_distance: Option<usize>,
    pub(super) p0_curve: &'a [usize],
    pub(super) unlock_count: usize,
    pub(super) priority_profile: PriorityProfile,
    pub(super) unlock_curve: &'a [usize],
    pub(super) step_priorities: &'a [StepPriority],
    pub(super) pagerank_bucket: Option<u64>,
    pub(super) stable_node_key: StableNodeKey,
}

#[derive(Clone, Copy)]
enum DecisiveComponent {
    CriticalDistance,
    P0Curve,
    UnlockCount,
    UnlockPriorityProfile,
    UnlockCurve,
    StepPrioritySequence,
    PageRankBucket,
    StableNodeKey,
}

struct KeyComparison {
    ordering: Ordering,
    decisive: DecisiveComponent,
}

pub(super) fn compare_ranking_keys(left: &RankingKey<'_>, right: &RankingKey<'_>) -> Ordering {
    compare_key_components(left, right).ordering
}

pub(super) fn compare(
    left: &EvaluatedCandidate<'_>,
    right: &EvaluatedCandidate<'_>,
) -> CandidateComparison {
    let left = ranking_key(left);
    let right = ranking_key(right);
    let comparison = compare_key_components(&left, &right);
    CandidateComparison {
        ordering: comparison.ordering,
        decisive: decisive_comparison(comparison.decisive, &left, &right),
    }
}

fn ranking_key<'candidate, 'issues>(
    candidate: &'candidate EvaluatedCandidate<'issues>,
) -> RankingKey<'candidate> {
    let data = candidate.data();
    RankingKey {
        mode: match candidate {
            EvaluatedCandidate::Normal(_) => RankingMode::Normal,
            EvaluatedCandidate::P0Ready(_) => RankingMode::P0Ready,
            EvaluatedCandidate::CriticalRoute { .. } => RankingMode::P0Route,
        },
        critical_distance: match candidate {
            EvaluatedCandidate::CriticalRoute { route, .. } => Some(route.distance().get()),
            EvaluatedCandidate::Normal(_) | EvaluatedCandidate::P0Ready(_) => None,
        },
        p0_curve: &data.p0_curve,
        unlock_count: data.unlocks.len(),
        priority_profile: data.priority_profile,
        unlock_curve: &data.unlock_curve,
        step_priorities: &data.step_priorities,
        pagerank_bucket: data.pagerank_bucket,
        stable_node_key: data.issue.stable_node_key(),
    }
}

fn compare_key_components(left: &RankingKey<'_>, right: &RankingKey<'_>) -> KeyComparison {
    assert_eq!(
        left.mode, right.mode,
        "one search compares candidates from a single typed mode"
    );
    if left.mode == RankingMode::P0Route && left.critical_distance != right.critical_distance {
        return KeyComparison {
            ordering: right.critical_distance.cmp(&left.critical_distance),
            decisive: DecisiveComponent::CriticalDistance,
        };
    }
    if left.mode.is_p0() && left.p0_curve != right.p0_curve {
        return KeyComparison {
            ordering: left.p0_curve.cmp(right.p0_curve),
            decisive: DecisiveComponent::P0Curve,
        };
    }
    if left.unlock_count != right.unlock_count {
        return KeyComparison {
            ordering: left.unlock_count.cmp(&right.unlock_count),
            decisive: DecisiveComponent::UnlockCount,
        };
    }
    if left.priority_profile != right.priority_profile {
        return KeyComparison {
            ordering: left
                .priority_profile
                .as_array()
                .cmp(&right.priority_profile.as_array()),
            decisive: DecisiveComponent::UnlockPriorityProfile,
        };
    }
    if left.unlock_curve != right.unlock_curve {
        return KeyComparison {
            ordering: left.unlock_curve.cmp(right.unlock_curve),
            decisive: DecisiveComponent::UnlockCurve,
        };
    }
    if left.step_priorities != right.step_priorities {
        return KeyComparison {
            ordering: left.step_priorities.cmp(right.step_priorities),
            decisive: DecisiveComponent::StepPrioritySequence,
        };
    }
    if left.pagerank_bucket != right.pagerank_bucket {
        return KeyComparison {
            ordering: left.pagerank_bucket.cmp(&right.pagerank_bucket),
            decisive: DecisiveComponent::PageRankBucket,
        };
    }
    KeyComparison {
        ordering: right.stable_node_key.cmp(&left.stable_node_key),
        decisive: DecisiveComponent::StableNodeKey,
    }
}

fn decisive_comparison(
    component: DecisiveComponent,
    left: &RankingKey<'_>,
    right: &RankingKey<'_>,
) -> DecisiveComparison {
    match component {
        DecisiveComponent::CriticalDistance => DecisiveComparison::CriticalDistance {
            left: left.critical_distance.unwrap_or_default(),
            right: right.critical_distance.unwrap_or_default(),
        },
        DecisiveComponent::P0Curve => DecisiveComparison::P0Curve {
            left: left.p0_curve.to_vec(),
            right: right.p0_curve.to_vec(),
        },
        DecisiveComponent::UnlockCount => DecisiveComparison::UnlockCount {
            left: left.unlock_count,
            right: right.unlock_count,
        },
        DecisiveComponent::UnlockPriorityProfile => DecisiveComparison::UnlockPriorityProfile {
            left: left.priority_profile,
            right: right.priority_profile,
        },
        DecisiveComponent::UnlockCurve => DecisiveComparison::UnlockCurve {
            left: left.unlock_curve.to_vec(),
            right: right.unlock_curve.to_vec(),
        },
        DecisiveComponent::StepPrioritySequence => DecisiveComparison::StepPrioritySequence {
            left: left.step_priorities.to_vec(),
            right: right.step_priorities.to_vec(),
        },
        DecisiveComponent::PageRankBucket => DecisiveComparison::PageRankBucket {
            left: left.pagerank_bucket.unwrap_or_default(),
            right: right.pagerank_bucket.unwrap_or_default(),
        },
        DecisiveComponent::StableNodeKey => DecisiveComparison::StableNodeKey {
            left: left.stable_node_key,
            right: right.stable_node_key,
        },
    }
}
