use std::collections::{BTreeMap, BTreeSet};

use crate::{
    model::{BlockerScope, Issue},
    operational::{IssueState, OperationalGraph},
};

pub(super) fn cyclic_components(
    graph: &OperationalGraph<'_>,
    issues: &BTreeMap<u64, &Issue>,
) -> Vec<Vec<u64>> {
    let open_nodes: Vec<_> = issues
        .iter()
        .filter(|(number, _)| graph.issue_state(**number) == Some(IssueState::Open))
        .map(|(number, _)| *number)
        .collect();
    let open_set: BTreeSet<_> = open_nodes.iter().copied().collect();
    let mut adjacency = BTreeMap::<u64, Vec<u64>>::new();
    let mut reverse = BTreeMap::<u64, Vec<u64>>::new();
    for node in &open_nodes {
        adjacency.insert(*node, Vec::new());
        reverse.insert(*node, Vec::new());
    }
    for blocked in &open_nodes {
        for dependency in graph.dependencies_for(*blocked) {
            if !matches!(dependency.blocker.scope, BlockerScope::Internal)
                || !open_set.contains(&dependency.blocker.number)
            {
                continue;
            }
            adjacency
                .entry(*blocked)
                .or_default()
                .push(dependency.blocker.number);
            reverse
                .entry(dependency.blocker.number)
                .or_default()
                .push(*blocked);
        }
    }
    for neighbors in adjacency.values_mut().chain(reverse.values_mut()) {
        neighbors.sort_unstable();
        neighbors.dedup();
    }

    let order = finish_order(&open_nodes, &adjacency);
    let mut assigned = BTreeSet::new();
    let mut components = Vec::new();
    for root in order.into_iter().rev() {
        if !assigned.insert(root) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            component.push(node);
            if let Some(neighbors) = reverse.get(&node) {
                for neighbor in neighbors.iter().rev() {
                    if assigned.insert(*neighbor) {
                        stack.push(*neighbor);
                    }
                }
            }
        }
        component.sort_unstable();
        let cyclic = component.len() > 1
            || component
                .first()
                .is_some_and(|node| adjacency[node].contains(node));
        if cyclic {
            components.push(component);
        }
    }
    components.sort();
    components
}

fn finish_order(nodes: &[u64], adjacency: &BTreeMap<u64, Vec<u64>>) -> Vec<u64> {
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for root in nodes {
        if !visited.insert(*root) {
            continue;
        }
        let mut stack = vec![(*root, 0_usize)];
        while let Some((node, next_index)) = stack.last().copied() {
            let neighbors = &adjacency[&node];
            if next_index < neighbors.len() {
                stack.last_mut().expect("stack is not empty").1 += 1;
                let neighbor = neighbors[next_index];
                if visited.insert(neighbor) {
                    stack.push((neighbor, 0));
                }
            } else {
                stack.pop();
                order.push(node);
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BlockerIdentity, Dependency, IssueIdentity, Label, LocalReplica, REPLICA_SCHEMA_VERSION,
    };

    #[test]
    fn finds_self_loops_and_distinct_connected_cycles_but_ignores_closed_history() {
        let issues = vec![
            issue(1, "open"),
            issue(2, "open"),
            issue(3, "open"),
            issue(4, "open"),
            issue(5, "open"),
            issue(6, "closed"),
            issue(7, "closed"),
        ];
        let dependencies = vec![
            dependency(1, 1),
            dependency(2, 3),
            dependency(3, 2),
            dependency(4, 5),
            dependency(5, 4),
            dependency(4, 3),
            dependency(6, 7),
            dependency(7, 6),
        ];
        let replica = replica(issues, dependencies);
        let graph = OperationalGraph::prepare(&replica);
        let issue_index = replica
            .issues
            .iter()
            .map(|issue| (issue.number, issue))
            .collect();

        assert_eq!(
            cyclic_components(&graph, &issue_index),
            vec![vec![1], vec![2, 3], vec![4, 5]]
        );
    }

    #[test]
    fn traverses_a_deep_acyclic_graph_iteratively() {
        const NODE_COUNT: u64 = 10_000;
        let issues = (1..=NODE_COUNT)
            .map(|number| issue(number, "open"))
            .collect();
        let dependencies = (2..=NODE_COUNT)
            .map(|number| dependency(number, number - 1))
            .collect();
        let replica = replica(issues, dependencies);
        let graph = OperationalGraph::prepare(&replica);
        let issue_index = replica
            .issues
            .iter()
            .map(|issue| (issue.number, issue))
            .collect();

        assert!(cyclic_components(&graph, &issue_index).is_empty());
    }

    fn replica(issues: Vec<Issue>, dependencies: Vec<Dependency>) -> LocalReplica {
        LocalReplica {
            relationships: Default::default(),
            schema_version: REPLICA_SCHEMA_VERSION.to_owned(),
            repository: "owner/repo".to_owned(),
            synced_at: "2026-08-07T00:00:00Z".to_owned(),
            input_hash: "unused-by-operational-analysis".to_owned(),
            repository_labels: Some(Vec::<Label>::new()),
            sync: Default::default(),
            issues,
            dependencies,
        }
    }

    fn issue(number: u64, state: &str) -> Issue {
        Issue {
            identity: crate::model::IssueIdentityState::GitHub,
            id: number,
            node_id: format!("ISSUE_{number}"),
            number,
            url: format!("https://github.com/owner/repo/issues/{number}"),
            title: format!("Issue {number}"),
            body: String::new(),
            state: state.to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            updated_at: "2026-08-07T00:00:00Z".to_owned(),
            closed_at: (state == "closed").then(|| "2026-08-07T00:00:00Z".to_owned()),
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
