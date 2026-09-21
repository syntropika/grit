use std::collections::{BTreeMap, BTreeSet};

pub(super) fn strongly_connected_components(
    nodes: &[u64],
    edges: &BTreeSet<(u64, u64)>,
) -> Vec<Vec<u64>> {
    let mut adjacency = BTreeMap::<u64, Vec<u64>>::new();
    let mut reverse = BTreeMap::<u64, Vec<u64>>::new();
    for node in nodes {
        adjacency.insert(*node, Vec::new());
        reverse.insert(*node, Vec::new());
    }
    for (source, target) in edges {
        adjacency.entry(*source).or_default().push(*target);
        reverse.entry(*target).or_default().push(*source);
    }
    for neighbors in adjacency.values_mut().chain(reverse.values_mut()) {
        neighbors.sort_unstable();
        neighbors.dedup();
    }

    let mut visited = BTreeSet::new();
    let mut finish_order = Vec::new();
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
                finish_order.push(node);
            }
        }
    }

    let mut assigned = BTreeSet::new();
    let mut components = Vec::new();
    for root in finish_order.into_iter().rev() {
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
        components.push(component);
    }
    components.sort_by_key(|component| component[0]);
    components
}

pub(super) fn cyclic_issue_numbers(
    components: &[Vec<u64>],
    edges: &BTreeSet<(u64, u64)>,
) -> BTreeSet<u64> {
    components
        .iter()
        .filter(|component| {
            component.len() > 1
                || component
                    .first()
                    .is_some_and(|number| edges.contains(&(*number, *number)))
        })
        .flatten()
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_self_loops_and_distinct_connected_cycles_without_closed_history() {
        let open_nodes = vec![1, 2, 3, 4, 5];
        let edges = BTreeSet::from([
            (1, 1),
            (2, 3),
            (3, 2),
            (4, 5),
            (5, 4),
            (4, 3),
            (6, 7),
            (7, 6),
        ]);

        let components = strongly_connected_components(&open_nodes, &edges);

        assert_eq!(components, vec![vec![1], vec![2, 3], vec![4, 5]]);
        assert_eq!(
            cyclic_issue_numbers(&components, &edges),
            BTreeSet::from([1, 2, 3, 4, 5])
        );
    }

    #[test]
    fn traverses_a_deep_acyclic_graph_without_recursion() {
        const NODE_COUNT: u64 = 10_000;
        let nodes: Vec<_> = (1..=NODE_COUNT).collect();
        let edges: BTreeSet<_> = (2..=NODE_COUNT)
            .map(|number| (number, number - 1))
            .collect();

        let components = strongly_connected_components(&nodes, &edges);

        assert_eq!(components.len(), NODE_COUNT as usize);
        assert!(cyclic_issue_numbers(&components, &edges).is_empty());
    }
}
