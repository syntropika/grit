use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::operational::OperationalGraph;

pub(super) const DAMPING: f64 = 0.85;
pub(super) const ITERATIONS: usize = 20;
pub(super) const BUCKET_SCALE: f64 = 1_000_000.0;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PageRank {
    buckets: BTreeMap<u64, u64>,
}

impl PageRank {
    pub(super) fn calculate(graph: &OperationalGraph<'_>) -> Option<Self> {
        let components = graph.components();
        if components.is_empty() {
            return None;
        }
        let mut component_by_issue = BTreeMap::new();
        for (component_index, component) in components.iter().enumerate() {
            for issue_number in component {
                component_by_issue.insert(*issue_number, component_index);
            }
        }
        let mut outgoing = vec![BTreeSet::new(); components.len()];
        for (dependent, blocker) in graph.internal_open_edges() {
            let source = component_by_issue[dependent];
            let target = component_by_issue[blocker];
            if source != target {
                outgoing[source].insert(target);
            }
        }

        let node_count = components.len();
        let denominator = node_count as f64;
        let mut scores = vec![1.0 / denominator; node_count];
        for _ in 0..ITERATIONS {
            let dangling_mass: f64 = scores
                .iter()
                .enumerate()
                .filter(|(index, _)| outgoing[*index].is_empty())
                .map(|(_, score)| *score)
                .sum();
            let base = (1.0 - DAMPING) / denominator + DAMPING * dangling_mass / denominator;
            let mut next = vec![base; node_count];
            for source in 0..node_count {
                if outgoing[source].is_empty() {
                    continue;
                }
                let share = DAMPING * scores[source] / outgoing[source].len() as f64;
                for target in &outgoing[source] {
                    next[*target] += share;
                }
            }
            if next.iter().any(|score| !score.is_finite() || *score < 0.0) {
                return None;
            }
            scores = next;
        }

        let mut buckets = BTreeMap::new();
        for (component_index, component) in components.iter().enumerate() {
            let bucket = (scores[component_index] * BUCKET_SCALE).floor() as u64;
            for issue_number in component {
                buckets.insert(*issue_number, bucket);
            }
        }
        Some(Self { buckets })
    }

    pub(super) fn bucket(&self, issue_number: u64) -> Option<u64> {
        self.buckets.get(&issue_number).copied()
    }

    pub(super) fn buckets(&self) -> &BTreeMap<u64, u64> {
        &self.buckets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BlockerIdentity, BlockerScope, Dependency, Issue, IssueIdentity, LocalReplica,
        REPLICA_SCHEMA_VERSION,
    };

    #[test]
    fn condenses_cycles_deduplicates_component_edges_and_uses_the_fixed_iterations() {
        let replica = LocalReplica {
            schema_version: REPLICA_SCHEMA_VERSION.to_owned(),
            repository: "owner/repo".to_owned(),
            synced_at: "2026-08-07T00:00:00Z".to_owned(),
            input_hash: "unused".to_owned(),
            repository_labels: Some(Vec::new()),
            sync: Default::default(),
            issues: vec![issue(1), issue(2), issue(3)],
            dependencies: vec![
                dependency(2, 3),
                dependency(3, 2),
                dependency(2, 1),
                dependency(3, 1),
                dependency(3, 1),
            ],
        };
        let graph = OperationalGraph::prepare(&replica);
        let pagerank = PageRank::calculate(&graph).expect("PageRank is available");

        assert_eq!(pagerank.bucket(1), Some(649_122));
        assert_eq!(pagerank.bucket(2), Some(350_877));
        assert_eq!(pagerank.bucket(3), Some(350_877));
        assert_eq!(DAMPING, 0.85);
        assert_eq!(ITERATIONS, 20);
        assert_eq!(BUCKET_SCALE, 1_000_000.0);
    }

    fn issue(number: u64) -> Issue {
        Issue {
            id: number,
            node_id: format!("ISSUE_{number}"),
            number,
            url: format!("https://github.com/owner/repo/issues/{number}"),
            title: format!("Issue {number}"),
            body: String::new(),
            state: "open".to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            updated_at: "2026-08-07T00:00:00Z".to_owned(),
            closed_at: None,
            identity: crate::model::IssueIdentityState::GitHub,
        }
    }

    fn dependency(blocked: u64, blocker: u64) -> Dependency {
        Dependency {
            blocked: IssueIdentity {
                repository: "owner/repo".to_owned(),
                number: blocked,
                id: blocked,
                node_id: format!("ISSUE_{blocked}"),
            },
            blocker: BlockerIdentity {
                repository: "owner/repo".to_owned(),
                number: blocker,
                state: "open".to_owned(),
                scope: BlockerScope::Internal,
                id: Some(blocker),
                node_id: Some(format!("ISSUE_{blocker}")),
            },
        }
    }
}
