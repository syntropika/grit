use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use super::artifact::{ArtifactNode, GraphArtifact, IssueNodeStatus, NodeKey};

pub(super) const FULL_NETWORK_MAX_NODES: usize = 5_000;
pub(super) const FULL_NETWORK_MAX_EDGES: usize = 20_000;
pub(super) const INITIAL_NETWORK_MAX_NODES: usize = 500;

#[derive(Serialize)]
pub(super) struct GraphPresentation {
    schema_version: &'static str,
    mode: PresentationMode,
    full_network_limits: FullNetworkLimits,
    initial_network_node_limit: usize,
    initial_node_keys: Vec<NodeKey>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PresentationMode {
    Full,
    Constrained,
}

#[derive(Serialize)]
struct FullNetworkLimits {
    nodes: usize,
    edges: usize,
}

pub(super) fn build(artifact: &GraphArtifact) -> GraphPresentation {
    build_with_limits(
        artifact,
        FULL_NETWORK_MAX_NODES,
        FULL_NETWORK_MAX_EDGES,
        INITIAL_NETWORK_MAX_NODES,
    )
}

fn build_with_limits(
    artifact: &GraphArtifact,
    maximum_nodes: usize,
    maximum_edges: usize,
    initial_node_limit: usize,
) -> GraphPresentation {
    let constrained = requires_constrained_view(
        artifact.nodes.len(),
        artifact.edges.len(),
        maximum_nodes,
        maximum_edges,
    );
    let initial_node_keys = if constrained {
        initial_nodes(artifact, initial_node_limit)
    } else {
        Vec::new()
    };
    GraphPresentation {
        schema_version: "grit.graph-presentation/v1",
        mode: if constrained {
            PresentationMode::Constrained
        } else {
            PresentationMode::Full
        },
        full_network_limits: FullNetworkLimits {
            nodes: maximum_nodes,
            edges: maximum_edges,
        },
        initial_network_node_limit: initial_node_limit,
        initial_node_keys,
    }
}

fn requires_constrained_view(
    node_count: usize,
    edge_count: usize,
    maximum_nodes: usize,
    maximum_edges: usize,
) -> bool {
    node_count > maximum_nodes || edge_count > maximum_edges
}

fn initial_nodes(artifact: &GraphArtifact, limit: usize) -> Vec<NodeKey> {
    if limit == 0 {
        return Vec::new();
    }
    let mut neighbors = BTreeMap::<NodeKey, Vec<NodeKey>>::new();
    for edge in &artifact.edges {
        neighbors
            .entry(edge.blocked.clone())
            .or_default()
            .push(edge.blocker.clone());
        neighbors
            .entry(edge.blocker.clone())
            .or_default()
            .push(edge.blocked.clone());
    }
    for values in neighbors.values_mut() {
        values.sort();
        values.dedup();
    }

    let mut selected = BTreeSet::new();
    let mut queue = VecDeque::new();
    for node in &artifact.nodes {
        if matches!(
            node,
            ArtifactNode::Issue {
                status: IssueNodeStatus::Ready,
                ..
            }
        ) {
            if selected.insert(node.key().clone()) {
                queue.push_back(node.key().clone());
            }
            if selected.len() == limit {
                return selected.into_iter().collect();
            }
        }
    }
    if queue.is_empty()
        && let Some(first) = artifact.nodes.first()
    {
        selected.insert(first.key().clone());
        queue.push_back(first.key().clone());
    }

    while let Some(current) = queue.pop_front() {
        for neighbor in neighbors.get(&current).into_iter().flatten() {
            if selected.insert(neighbor.clone()) {
                queue.push_back(neighbor.clone());
                if selected.len() == limit {
                    return selected.into_iter().collect();
                }
            }
        }
    }
    for node in &artifact.nodes {
        selected.insert(node.key().clone());
        if selected.len() == limit {
            break;
        }
    }
    selected.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use crate::model::{
        BlockerIdentity, BlockerScope, Dependency, Issue, IssueIdentity, LocalReplica,
    };

    use super::*;

    #[test]
    fn constrains_above_limits_and_seeds_the_view_from_ready_work() {
        let replica = LocalReplica::build(
            "acme/widgets".to_owned(),
            "2026-08-07T00:00:00Z".to_owned(),
            Vec::new(),
            (1..=4).map(|number| issue(number, "open")).collect(),
            Vec::new(),
        )
        .expect("synthetic replica");
        let artifact = crate::graph::artifact::build(
            &replica,
            crate::operational::ExecutionScope::Available,
            crate::ranking::DEFAULT_HORIZON,
        )
        .expect("graph artifact");
        let presentation = build_with_limits(&artifact, 3, usize::MAX, 2);
        let value = serde_json::to_value(presentation).expect("presentation JSON");

        assert_eq!(value["schema_version"], "grit.graph-presentation/v1");
        assert_eq!(value["mode"], "constrained");
        assert_eq!(value["initial_network_node_limit"], 2);
        assert_eq!(
            value["initial_node_keys"],
            serde_json::json!(["acme/widgets#1", "acme/widgets#2"])
        );
        assert!(!requires_constrained_view(5_000, 20_000, 5_000, 20_000));
        assert!(requires_constrained_view(5_001, 20_000, 5_000, 20_000));
        assert!(requires_constrained_view(5_000, 20_001, 5_000, 20_000));
    }

    #[test]
    fn initial_view_traverses_a_dependency_chain_from_ready_work() {
        let artifact = artifact(
            vec![issue(1, "open"), issue(2, "open"), issue(3, "open")],
            vec![dependency(2, 1), dependency(3, 2)],
        );

        assert_eq!(keys(initial_nodes(&artifact, 3)), ["#1", "#2", "#3"]);
    }

    #[test]
    fn initial_view_fills_from_disconnected_nodes_after_traversal() {
        let artifact = artifact(
            vec![issue(1, "open"), issue(2, "open"), issue(3, "closed")],
            vec![dependency(2, 1)],
        );

        assert_eq!(keys(initial_nodes(&artifact, 3)), ["#1", "#2", "#3"]);
    }

    #[test]
    fn initial_view_falls_back_to_the_first_stable_key_without_ready_work() {
        let artifact = artifact(vec![issue(2, "closed"), issue(1, "closed")], Vec::new());

        assert_eq!(keys(initial_nodes(&artifact, 2)), ["#1", "#2"]);
    }

    #[test]
    fn initial_view_deduplicates_and_orders_neighbors_deterministically() {
        let issues = vec![issue(1, "open"), issue(2, "open"), issue(10, "open")];
        let first = artifact(
            issues.clone(),
            vec![dependency(2, 1), dependency(10, 1), dependency(2, 1)],
        );
        let second = artifact(
            issues,
            vec![dependency(10, 1), dependency(2, 1), dependency(10, 1)],
        );

        assert_eq!(keys(initial_nodes(&first, 2)), ["#1", "#2"]);
        assert_eq!(
            keys(initial_nodes(&first, 2)),
            keys(initial_nodes(&second, 2))
        );
    }

    fn artifact(issues: Vec<Issue>, dependencies: Vec<Dependency>) -> GraphArtifact {
        let replica = LocalReplica::build(
            "acme/widgets".to_owned(),
            "2026-08-07T00:00:00Z".to_owned(),
            Vec::new(),
            issues,
            dependencies,
        )
        .expect("synthetic replica");
        crate::graph::artifact::build(
            &replica,
            crate::operational::ExecutionScope::Available,
            crate::ranking::DEFAULT_HORIZON,
        )
        .expect("graph artifact")
    }

    fn keys(values: Vec<NodeKey>) -> Vec<String> {
        values
            .into_iter()
            .map(|key| {
                let key = key.to_string();
                format!("#{}", key.rsplit_once('#').expect("Stable node key").1)
            })
            .collect()
    }

    fn issue(number: u64, state: &str) -> Issue {
        Issue {
            id: number,
            node_id: format!("I_{number}"),
            number,
            url: format!("https://github.com/acme/widgets/issues/{number}"),
            title: format!("Issue {number}"),
            body: String::new(),
            state: state.to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            created_at: "2026-08-01T00:00:00Z".to_owned(),
            updated_at: "2026-08-01T00:00:00Z".to_owned(),
            closed_at: (state == "closed").then(|| "2026-08-02T00:00:00Z".to_owned()),
        }
    }

    fn dependency(blocked: u64, blocker: u64) -> Dependency {
        Dependency {
            blocked: IssueIdentity {
                repository: "acme/widgets".to_owned(),
                number: blocked,
                id: blocked,
                node_id: format!("I_{blocked}"),
            },
            blocker: BlockerIdentity {
                repository: "acme/widgets".to_owned(),
                number: blocker,
                state: "open".to_owned(),
                scope: BlockerScope::Internal,
                id: Some(blocker),
                node_id: Some(format!("I_{blocker}")),
            },
        }
    }
}
