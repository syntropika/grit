use std::collections::{BTreeSet, VecDeque};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{BlockerResolution, OperationalGraph, PreparedRepository};

pub(crate) const TRAVERSAL_WORK_LIMIT: usize = 50_000;
pub(crate) const EXAMPLE_LIMIT: usize = 5;

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DependencyImpact {
    pub(crate) ready_now: bool,
    pub(crate) direct_dependents: usize,
    pub(crate) downstream: ImpactCount,
    pub(crate) immediate_unlocks: Option<ImmediateOutcomes>,
    pub(crate) still_blocked: Option<BlockedOutcomes>,
    pub(crate) chain_depth: ChainDepth,
    pub(crate) explanation: String,
    pub(crate) pending: bool,
    traversal_work_limit: usize,
    example_limit: usize,
}

impl DependencyImpact {
    pub(crate) fn human_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("Dependency impact: {}", self.explanation),
            format!("Direct open dependents: {}", self.direct_dependents),
            format!(
                "Longest downstream chain: {}",
                self.chain_depth.description()
            ),
        ];
        if !self.downstream.complete {
            lines.push(
                "Downstream counts are lower bounds; some work was not inspected.".to_owned(),
            );
        }
        if let Some(unlocks) = &self.immediate_unlocks
            && !unlocks.examples.is_empty()
        {
            lines.push(format!(
                "Would become ready (showing {} of {}): {}",
                unlocks.examples.len(),
                unlocks.count,
                unlocks.examples.join(", ")
            ));
        }
        if let Some(blocked) = &self.still_blocked {
            for outcome in &blocked.examples {
                let blockers: Vec<_> = outcome
                    .blockers
                    .iter()
                    .map(|blocker| {
                        format!(
                            "{}{}{}",
                            blocker.key.as_deref().unwrap_or("Unknown internal Issue"),
                            if blocker.external { " (external)" } else { "" },
                            if blocker.unknown {
                                " (unknown state)"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect();
                lines.push(format!(
                    "Still blocked: {}{}; remaining blockers (showing {} of {}): {}",
                    outcome.key,
                    if outcome.cyclic { " (cycle)" } else { "" },
                    blockers.len(),
                    outcome.blocker_count,
                    blockers.join(", ")
                ));
            }
        }
        lines
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImpactCount {
    pub(crate) count: usize,
    pub(crate) complete: bool,
}

impl ImpactCount {
    fn description(&self) -> String {
        if self.complete {
            self.count.to_string()
        } else {
            format!("at least {}", self.count)
        }
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImmediateOutcomes {
    pub(crate) count: usize,
    pub(crate) examples: Vec<String>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlockedOutcomes {
    pub(crate) total: ImpactCount,
    pub(crate) examples: Vec<BlockedOutcome>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlockedOutcome {
    pub(crate) key: String,
    pub(crate) cyclic: bool,
    pub(crate) blocker_count: usize,
    pub(crate) blockers: Vec<RemainingBlocker>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemainingBlocker {
    pub(crate) key: Option<String>,
    pub(crate) external: bool,
    pub(crate) unknown: bool,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChainDepth {
    Finite { edges: usize },
    Unavailable { reason: DepthUnavailableReason },
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DepthUnavailableReason {
    ReachableCycle,
}

impl ChainDepth {
    pub(crate) fn description(&self) -> String {
        match self {
            Self::Finite { edges } => format!("{edges} dependency steps (not a duration)"),
            Self::Unavailable { .. } => "unavailable: a downstream cycle is reachable".to_owned(),
        }
    }
}

/// Shared diagnostic preparation. Ranking and public export do not consume these metrics.
pub(crate) struct ImpactAnalysis<'graph, 'issues> {
    graph: &'graph OperationalGraph<'issues>,
    repository: &'graph str,
    pending: bool,
    dependents: Vec<Vec<usize>>,
    ready: Vec<bool>,
    immediate: Vec<Vec<usize>>,
    depths: Vec<Option<usize>>,
}

impl<'graph, 'issues> ImpactAnalysis<'graph, 'issues> {
    pub(crate) fn prepare(prepared: &'graph PreparedRepository<'issues>) -> Self {
        let graph = prepared.graph();
        let numbers = graph.open_numbers();
        let mut dependents = vec![Vec::new(); numbers.len()];
        let mut blockers = vec![Vec::new(); numbers.len()];
        for (dependent, blocker) in graph.internal_open_edges() {
            let dependent = graph.open_index(*dependent).expect("open dependent");
            let blocker = graph.open_index(*blocker).expect("open blocker");
            dependents[blocker].push(dependent);
            blockers[dependent].push(blocker);
        }
        for adjacent in &mut dependents {
            adjacent.sort_unstable();
        }
        let ready: Vec<_> = numbers
            .iter()
            .map(|number| graph.is_ready(*number))
            .collect();
        let mut immediate = vec![Vec::new(); numbers.len()];
        for (index, number) in numbers.iter().enumerate() {
            if graph.cyclic_numbers().contains(number) {
                continue;
            }
            let mut sole = None;
            let mut opaque_or_multiple = false;
            for dependency in graph.dependencies_for(*number) {
                match graph.blocker_resolution(dependency) {
                    BlockerResolution::Satisfied => {}
                    BlockerResolution::InternalOpen(blocker) => match sole {
                        None => sole = Some(blocker),
                        Some(previous) if previous == blocker => {}
                        Some(_) => opaque_or_multiple = true,
                    },
                    _ => opaque_or_multiple = true,
                }
            }
            if !opaque_or_multiple && let Some(sole) = sole {
                let blocker = graph.open_index(sole).expect("open blocker");
                if ready[blocker] {
                    immediate[blocker].push(index);
                }
            }
        }

        // Reverse topological processing leaves cycles and their ancestors unresolved.
        let mut remaining: Vec<_> = dependents.iter().map(Vec::len).collect();
        let mut depth = vec![0; numbers.len()];
        let mut depths = vec![None; numbers.len()];
        let mut queue: VecDeque<_> = remaining
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(index))
            .collect();
        while let Some(index) = queue.pop_front() {
            depths[index] = Some(depth[index]);
            for blocker in &blockers[index] {
                depth[*blocker] = depth[*blocker].max(depth[index] + 1);
                remaining[*blocker] -= 1;
                if remaining[*blocker] == 0 {
                    queue.push_back(*blocker);
                }
            }
        }
        Self {
            graph,
            repository: &prepared.working().replica().repository,
            pending: prepared.working().is_pending(),
            dependents,
            ready,
            immediate,
            depths,
        }
    }

    pub(crate) fn for_issue(&self, number: u64) -> Option<DependencyImpact> {
        self.analyze(number, TRAVERSAL_WORK_LIMIT)
    }

    fn analyze(&self, number: u64, work_limit: usize) -> Option<DependencyImpact> {
        let root = self.graph.open_index(number)?;
        let mut seen = vec![false; self.dependents.len()];
        seen[root] = true;
        let mut reached = Vec::new();
        let mut queue = VecDeque::from([root]);
        let mut work = 0;
        let mut complete = true;
        'traverse: while let Some(index) = queue.pop_front() {
            if work == work_limit {
                complete = false;
                break;
            }
            work += 1;
            for dependent in &self.dependents[index] {
                if work == work_limit {
                    complete = false;
                    break 'traverse;
                }
                work += 1;
                if !seen[*dependent] {
                    seen[*dependent] = true;
                    reached.push(*dependent);
                    queue.push_back(*dependent);
                }
            }
        }
        reached.sort_unstable();
        let downstream = ImpactCount {
            count: reached.len(),
            complete,
        };
        let immediate_unlocks = self.ready[root].then(|| ImmediateOutcomes {
            count: self.immediate[root].len(),
            examples: self.immediate[root]
                .iter()
                .take(EXAMPLE_LIMIT)
                .map(|index| self.key(*index))
                .collect(),
        });
        let still_blocked = self.ready[root].then(|| {
            let blocked: Vec<_> = reached
                .iter()
                .copied()
                .filter(|index| self.immediate[root].binary_search(index).is_err())
                .collect();
            BlockedOutcomes {
                total: ImpactCount {
                    count: blocked.len(),
                    complete,
                },
                examples: blocked
                    .iter()
                    .take(EXAMPLE_LIMIT)
                    .map(|index| self.blocked_outcome(*index, number))
                    .collect(),
            }
        });
        let prefix = format!("Open downstream Issues: {}.", downstream.description());
        let explanation = match (&immediate_unlocks, &still_blocked) {
            (Some(unlocks), Some(blocked)) => format!(
                "{prefix} Completing this Issue would make {} ready; {} would still need other blockers resolved.",
                unlocks.count,
                blocked.total.description()
            ),
            _ => format!(
                "{prefix} This Issue is blocked; resolve its blockers before treating completion as an executable step."
            ),
        };
        Some(DependencyImpact {
            ready_now: self.ready[root],
            direct_dependents: self.dependents[root]
                .iter()
                .filter(|index| **index != root)
                .count(),
            downstream,
            immediate_unlocks,
            still_blocked,
            chain_depth: self.depths[root]
                .map(|edges| ChainDepth::Finite { edges })
                .unwrap_or(ChainDepth::Unavailable {
                    reason: DepthUnavailableReason::ReachableCycle,
                }),
            explanation,
            pending: self.pending,
            traversal_work_limit: work_limit,
            example_limit: EXAMPLE_LIMIT,
        })
    }

    fn key(&self, index: usize) -> String {
        self.graph
            .issue(self.graph.open_numbers()[index])
            .expect("open Issue")
            .display_key(self.repository)
    }

    fn blocked_outcome(&self, index: usize, completed: u64) -> BlockedOutcome {
        let number = self.graph.open_numbers()[index];
        let mut blockers = Vec::new();
        let mut identities = BTreeSet::new();
        for dependency in self.graph.dependencies_for(number) {
            let resolution = self.graph.blocker_resolution(dependency);
            if resolution == BlockerResolution::Satisfied
                || resolution == BlockerResolution::InternalOpen(completed)
            {
                continue;
            }
            if !identities.insert((
                dependency.blocker.repository.to_ascii_lowercase(),
                dependency.blocker.number,
            )) {
                continue;
            }
            let external = matches!(
                resolution,
                BlockerResolution::ExternalOpen | BlockerResolution::ExternalUnknown
            );
            let key = if external {
                Some(format!(
                    "{}#{}",
                    dependency.blocker.repository, dependency.blocker.number
                ))
            } else {
                self.graph
                    .issue(dependency.blocker.number)
                    .map(|issue| issue.display_key(self.repository))
            };
            blockers.push(RemainingBlocker {
                key,
                external,
                unknown: matches!(
                    resolution,
                    BlockerResolution::ExternalUnknown | BlockerResolution::InternalUnknown
                ),
            });
        }
        blockers.sort_by(|left, right| left.key.cmp(&right.key));
        let blocker_count = blockers.len();
        blockers.truncate(EXAMPLE_LIMIT);
        BlockedOutcome {
            key: self.key(index),
            cyclic: self.graph.cyclic_numbers().contains(&number),
            blocker_count,
            blockers,
        }
    }
}

#[cfg(test)]
#[path = "impact_tests.rs"]
mod tests;
