use std::collections::{BTreeMap, BTreeSet};

use super::{
    GraphError,
    artifact::{ArtifactEdge, ArtifactNode, LayerRole, NodeKey, Position},
};

pub(super) fn assign_dependency_layers(
    nodes: &mut [ArtifactNode],
    edges: &[ArtifactEdge],
) -> Result<(), GraphError> {
    let indexes: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.key().clone(), index))
        .collect();
    let mut open_blockers: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
    let mut opaque_boundary = BTreeSet::new();

    for edge in edges {
        let blocked_index = *indexes
            .get(&edge.blocked)
            .ok_or_else(|| GraphError::DanglingEndpoint(edge.blocked.to_string()))?;
        let blocker_index = *indexes
            .get(&edge.blocker)
            .ok_or_else(|| GraphError::DanglingEndpoint(edge.blocker.to_string()))?;
        if !nodes[blocked_index].is_open_issue() {
            continue;
        }
        let blocker = &nodes[blocker_index];
        match blocker.layer_role() {
            LayerRole::Satisfied => {}
            LayerRole::OpenIssue => open_blockers
                .entry(edge.blocked.clone())
                .or_default()
                .push(edge.blocker.clone()),
            LayerRole::Opaque => {
                opaque_boundary.insert(edge.blocked.clone());
            }
        }
    }
    for blockers in open_blockers.values_mut() {
        blockers.sort();
        blockers.dedup();
    }

    let mut layers = BTreeMap::<NodeKey, u32>::new();
    for node in nodes.iter() {
        if node.is_open_issue()
            && !opaque_boundary.contains(node.key())
            && open_blockers
                .get(node.key())
                .is_none_or(|blockers| blockers.is_empty())
        {
            layers.insert(node.key().clone(), 0);
        }
    }

    loop {
        let mut progressed = false;
        for node in nodes.iter() {
            if !node.is_open_issue()
                || layers.contains_key(node.key())
                || opaque_boundary.contains(node.key())
            {
                continue;
            }
            let Some(blockers) = open_blockers.get(node.key()) else {
                continue;
            };
            let blocker_layers: Option<Vec<_>> = blockers
                .iter()
                .map(|blocker| layers.get(blocker).copied())
                .collect();
            if let Some(blocker_layers) = blocker_layers {
                let layer = blocker_layers.into_iter().max().unwrap_or(0) + 1;
                layers.insert(node.key().clone(), layer);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    let mut rows = BTreeMap::<u32, u32>::new();
    let unresolved_x = i64::from(layers.values().copied().max().unwrap_or(0) + 1) * 320;
    let mut unresolved_row = 0_i64;
    for node in nodes {
        if let Some(layer) = layers.get(node.key()).copied() {
            let row = rows.entry(layer).or_default();
            *node.position_mut() = Position {
                layer: Some(layer),
                x: i64::from(layer) * 320,
                y: i64::from(*row) * 72,
            };
            *row += 1;
        } else {
            *node.position_mut() = Position {
                layer: None,
                x: unresolved_x,
                y: unresolved_row * 72,
            };
            unresolved_row += 1;
        }
    }
    Ok(())
}
