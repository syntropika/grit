use std::collections::{BTreeMap, HashMap, VecDeque};

use super::{
    GraphError,
    artifact::{ArtifactEdge, ArtifactNode, NodeKind},
    model::{NodeKey, Position},
};

pub(super) struct LayoutNode {
    key: NodeKey,
    eligible: bool,
    opaque_boundary: bool,
}

impl LayoutNode {
    pub(super) fn eligible(key: NodeKey, opaque_boundary: bool) -> Self {
        Self {
            key,
            eligible: true,
            opaque_boundary,
        }
    }

    fn excluded(key: NodeKey) -> Self {
        Self {
            key,
            eligible: false,
            opaque_boundary: false,
        }
    }
}

pub(super) struct LayoutDependency {
    blocked: NodeKey,
    blocker: NodeKey,
}

impl LayoutDependency {
    pub(super) fn new(blocked: NodeKey, blocker: NodeKey) -> Self {
        Self { blocked, blocker }
    }
}

pub(super) fn assign_artifact_dependency_layers(
    nodes: &mut [ArtifactNode],
    edges: &[ArtifactEdge],
) -> Result<(), GraphError> {
    let indexes: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.key.clone(), index))
        .collect();
    let mut layout_nodes: Vec<_> = nodes
        .iter()
        .map(|node| {
            if node.kind == NodeKind::Issue && node.state == "open" {
                LayoutNode::eligible(node.key.clone(), false)
            } else {
                LayoutNode::excluded(node.key.clone())
            }
        })
        .collect();
    let mut dependencies = Vec::new();
    for edge in edges {
        let blocked_index = *indexes
            .get(&edge.blocked)
            .ok_or_else(|| GraphError::DanglingEndpoint(edge.blocked.to_string()))?;
        let blocker_index = *indexes
            .get(&edge.blocker)
            .ok_or_else(|| GraphError::DanglingEndpoint(edge.blocker.to_string()))?;
        if nodes[blocked_index].kind != NodeKind::Issue || nodes[blocked_index].state != "open" {
            continue;
        }
        let blocker = &nodes[blocker_index];
        match (blocker.kind, blocker.state.as_str()) {
            (_, "closed") => {}
            (NodeKind::Issue, "open") => dependencies.push(LayoutDependency::new(
                edge.blocked.clone(),
                edge.blocker.clone(),
            )),
            (NodeKind::Issue | NodeKind::ExternalBlocker, _) => {
                layout_nodes[blocked_index].opaque_boundary = true;
            }
        }
    }
    let positions = dependency_positions(&layout_nodes, &dependencies)?;
    for node in nodes {
        node.position = positions
            .get(&node.key)
            .cloned()
            .ok_or_else(|| GraphError::DanglingEndpoint(node.key.to_string()))?;
    }
    Ok(())
}

pub(super) fn dependency_positions(
    nodes: &[LayoutNode],
    dependencies: &[LayoutDependency],
) -> Result<BTreeMap<NodeKey, Position>, GraphError> {
    let indexes: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.key.clone(), index))
        .collect();
    let mut indegree = vec![0_usize; nodes.len()];
    let mut dependents = vec![Vec::<usize>::new(); nodes.len()];
    for dependency in dependencies {
        let blocked = *indexes
            .get(&dependency.blocked)
            .ok_or_else(|| GraphError::DanglingEndpoint(dependency.blocked.to_string()))?;
        let blocker = *indexes
            .get(&dependency.blocker)
            .ok_or_else(|| GraphError::DanglingEndpoint(dependency.blocker.to_string()))?;
        if !nodes[blocked].eligible || !nodes[blocker].eligible {
            return Err(GraphError::InvalidField("layout Dependency"));
        }
        indegree[blocked] += 1;
        dependents[blocker].push(blocked);
    }

    let mut layers = vec![None; nodes.len()];
    let mut next_layer = vec![0_u32; nodes.len()];
    let mut queue = VecDeque::new();
    for (index, node) in nodes.iter().enumerate() {
        if node.eligible && !node.opaque_boundary && indegree[index] == 0 {
            layers[index] = Some(0);
            queue.push_back(index);
        }
    }
    while let Some(blocker) = queue.pop_front() {
        let blocker_layer = layers[blocker].expect("queued nodes have a Dependency layer");
        for &blocked in &dependents[blocker] {
            next_layer[blocked] = next_layer[blocked].max(blocker_layer + 1);
            indegree[blocked] -= 1;
            if indegree[blocked] == 0 && !nodes[blocked].opaque_boundary {
                layers[blocked] = Some(next_layer[blocked]);
                queue.push_back(blocked);
            }
        }
    }

    let max_layer = layers.iter().flatten().copied().max().unwrap_or(0);
    let unresolved_x = i64::from(max_layer + 1) * 320;
    let mut rows = BTreeMap::<u32, u32>::new();
    let mut unresolved_row = 0_i64;
    let mut positions = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let position = if let Some(layer) = layers[index] {
            let row = rows.entry(layer).or_default();
            let position = Position {
                layer: Some(layer),
                x: i64::from(layer) * 320,
                y: i64::from(*row) * 72,
            };
            *row += 1;
            position
        } else {
            let position = Position {
                layer: None,
                x: unresolved_x,
                y: unresolved_row * 72,
            };
            unresolved_row += 1;
            position
        };
        positions.insert(node.key.clone(), position);
    }
    Ok(positions)
}

#[cfg(test)]
mod tests {
    use super::{LayoutDependency, LayoutNode, dependency_positions};
    use crate::graph::model::NodeKey;

    #[test]
    fn reverse_numbered_chain_is_layered_without_repeated_graph_scans() {
        const NODE_COUNT: u64 = 20_000;
        let nodes: Vec<_> = (1..=NODE_COUNT)
            .map(|number| LayoutNode::eligible(NodeKey::new("acme/widgets", number), false))
            .collect();
        let dependencies: Vec<_> = (1..NODE_COUNT)
            .map(|blocked| {
                LayoutDependency::new(
                    NodeKey::new("acme/widgets", blocked),
                    NodeKey::new("acme/widgets", blocked + 1),
                )
            })
            .collect();

        let positions = dependency_positions(&nodes, &dependencies).expect("chain layout");

        assert_eq!(
            positions[&NodeKey::new("acme/widgets", NODE_COUNT)].layer,
            Some(0)
        );
        assert_eq!(
            positions[&NodeKey::new("acme/widgets", 1)].layer,
            Some(19_999)
        );
    }

    #[test]
    fn cycles_and_their_downstream_nodes_remain_unresolved() {
        let nodes: Vec<_> = (1..=3)
            .map(|number| LayoutNode::eligible(NodeKey::new("acme/widgets", number), false))
            .collect();
        let dependencies = vec![
            LayoutDependency::new(
                NodeKey::new("acme/widgets", 1),
                NodeKey::new("acme/widgets", 2),
            ),
            LayoutDependency::new(
                NodeKey::new("acme/widgets", 2),
                NodeKey::new("acme/widgets", 1),
            ),
            LayoutDependency::new(
                NodeKey::new("acme/widgets", 3),
                NodeKey::new("acme/widgets", 2),
            ),
        ];

        let positions = dependency_positions(&nodes, &dependencies).expect("cycle layout");

        assert!(positions.values().all(|position| position.layer.is_none()));
    }
}
