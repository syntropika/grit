use std::collections::{BTreeMap, BTreeSet};

use crate::model::{BlockerScope, Dependency, Issue, LocalReplica};

#[derive(Clone, Copy)]
pub(crate) enum ExecutionScope<'a> {
    Available,
    Assignee(&'a str),
}

impl ExecutionScope<'_> {
    pub(crate) fn contains(self, issue: &Issue) -> bool {
        match self {
            Self::Available => issue.assignees.is_empty(),
            Self::Assignee(assignee) => issue
                .assignees
                .iter()
                .any(|actor| actor.login.eq_ignore_ascii_case(assignee)),
        }
    }

    pub(crate) fn hash_key(self) -> (&'static str, Option<String>) {
        match self {
            Self::Available => ("available", None),
            Self::Assignee(assignee) => ("assignee", Some(assignee.to_ascii_lowercase())),
        }
    }
}

pub(crate) struct ReadyAnalysis<'a> {
    pub(crate) operational_issue_count: usize,
    pub(crate) ready_count: usize,
    pub(crate) assigned_ready_count: usize,
    pub(crate) blocked_count: usize,
    pub(crate) executable: Vec<&'a Issue>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum IssueState {
    Open,
    Closed,
    Unknown,
}

impl IssueState {
    pub(crate) fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("open") {
            Self::Open
        } else if value.eq_ignore_ascii_case("closed") {
            Self::Closed
        } else {
            Self::Unknown
        }
    }
}

pub(crate) struct OperationalGraph<'a> {
    issues: BTreeMap<u64, &'a Issue>,
    issue_states: BTreeMap<u64, IssueState>,
    dependencies_by_blocked: BTreeMap<u64, Vec<&'a Dependency>>,
    immediate_unlocks_by_blocker: BTreeMap<u64, Vec<u64>>,
    open_numbers: Vec<u64>,
    internal_open_edges: BTreeSet<(u64, u64)>,
    components: Vec<Vec<u64>>,
    cyclic_numbers: BTreeSet<u64>,
}

impl<'a> OperationalGraph<'a> {
    pub(crate) fn prepare(replica: &'a LocalReplica) -> Self {
        let issues: BTreeMap<_, _> = replica
            .issues
            .iter()
            .map(|issue| (issue.number, issue))
            .collect();
        let issue_states: BTreeMap<_, _> = issues
            .iter()
            .map(|(number, issue)| (*number, IssueState::parse(&issue.state)))
            .collect();
        let open_numbers: Vec<_> = issue_states
            .iter()
            .filter(|(_, state)| **state == IssueState::Open)
            .map(|(number, _)| *number)
            .collect();
        let open_set: BTreeSet<_> = open_numbers.iter().copied().collect();
        let mut dependencies_by_blocked = BTreeMap::<u64, Vec<&Dependency>>::new();
        let mut internal_open_edges = BTreeSet::new();
        for dependency in &replica.dependencies {
            if !dependency
                .blocked
                .repository
                .eq_ignore_ascii_case(&replica.repository)
            {
                continue;
            }
            dependencies_by_blocked
                .entry(dependency.blocked.number)
                .or_default()
                .push(dependency);
            if matches!(dependency.blocker.scope, BlockerScope::Internal)
                && open_set.contains(&dependency.blocked.number)
                && open_set.contains(&dependency.blocker.number)
            {
                internal_open_edges.insert((dependency.blocked.number, dependency.blocker.number));
            }
        }
        for dependencies in dependencies_by_blocked.values_mut() {
            dependencies.sort_by(|left, right| {
                left.blocker
                    .repository
                    .to_ascii_lowercase()
                    .cmp(&right.blocker.repository.to_ascii_lowercase())
                    .then_with(|| left.blocker.number.cmp(&right.blocker.number))
            });
        }
        let components = strongly_connected_components(&open_numbers, &internal_open_edges);
        let cyclic_numbers = cyclic_issue_numbers(&components, &internal_open_edges);
        let mut immediate_unlocks_by_blocker = BTreeMap::<u64, Vec<u64>>::new();
        for dependent in &open_numbers {
            if cyclic_numbers.contains(dependent) {
                continue;
            }
            let unsatisfied = unsatisfied_blockers(
                dependencies_by_blocked
                    .get(dependent)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &issue_states,
            );
            if let [UnsatisfiedBlocker::Internal(blocker)] = unsatisfied.as_slice() {
                immediate_unlocks_by_blocker
                    .entry(*blocker)
                    .or_default()
                    .push(*dependent);
            }
        }
        Self {
            issues,
            issue_states,
            dependencies_by_blocked,
            immediate_unlocks_by_blocker,
            open_numbers,
            internal_open_edges,
            components,
            cyclic_numbers,
        }
    }

    pub(crate) fn issue(&self, number: u64) -> Option<&'a Issue> {
        self.issues.get(&number).copied()
    }

    pub(crate) fn issue_state(&self, number: u64) -> Option<IssueState> {
        self.issue_states.get(&number).copied()
    }

    pub(crate) fn dependencies_for(&self, number: u64) -> &[&'a Dependency] {
        self.dependencies_by_blocked
            .get(&number)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn immediate_unlocks_for(&self, number: u64) -> &[u64] {
        self.immediate_unlocks_by_blocker
            .get(&number)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn internal_open_edges(&self) -> &BTreeSet<(u64, u64)> {
        &self.internal_open_edges
    }

    pub(crate) fn components(&self) -> &[Vec<u64>] {
        &self.components
    }

    pub(crate) fn cyclic_numbers(&self) -> &BTreeSet<u64> {
        &self.cyclic_numbers
    }

    pub(crate) fn unknown_blocker_count(&self) -> usize {
        self.open_numbers
            .iter()
            .filter(|number| {
                self.dependencies_for(**number).iter().any(|dependency| {
                    match dependency.blocker.scope {
                        BlockerScope::External => {
                            IssueState::parse(&dependency.blocker.state) == IssueState::Unknown
                        }
                        BlockerScope::Internal => self
                            .issue_state(dependency.blocker.number)
                            .is_none_or(|state| state == IssueState::Unknown),
                    }
                })
            })
            .count()
    }

    pub(crate) fn is_ready(&self, number: u64) -> bool {
        if self.issue_state(number) != Some(IssueState::Open)
            || self.cyclic_numbers.contains(&number)
        {
            return false;
        }
        unsatisfied_blockers(self.dependencies_for(number), &self.issue_states).is_empty()
    }

    pub(crate) fn analyze_ready(&self, scope: ExecutionScope<'_>) -> ReadyAnalysis<'a> {
        let mut ready: Vec<_> = self
            .open_numbers
            .iter()
            .filter(|number| self.is_ready(**number))
            .filter_map(|number| self.issue(*number))
            .collect();
        ready.sort_by_key(|issue| issue.number);
        let assigned_ready_count = ready
            .iter()
            .filter(|issue| !issue.assignees.is_empty())
            .count();
        let executable = ready
            .iter()
            .copied()
            .filter(|issue| scope.contains(issue))
            .collect();
        let operational_issue_count = self.open_numbers.len();
        let ready_count = ready.len();
        ReadyAnalysis {
            operational_issue_count,
            ready_count,
            assigned_ready_count,
            blocked_count: operational_issue_count - ready_count,
            executable,
        }
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum UnsatisfiedBlocker {
    Internal(u64),
    Opaque(String, u64),
}

fn unsatisfied_blockers(
    dependencies: &[&Dependency],
    issue_states: &BTreeMap<u64, IssueState>,
) -> Vec<UnsatisfiedBlocker> {
    dependencies
        .iter()
        .filter_map(|dependency| match dependency.blocker.scope {
            BlockerScope::Internal => match issue_states.get(&dependency.blocker.number) {
                Some(IssueState::Closed) => None,
                Some(IssueState::Open) => {
                    Some(UnsatisfiedBlocker::Internal(dependency.blocker.number))
                }
                Some(IssueState::Unknown) | None => Some(UnsatisfiedBlocker::Opaque(
                    dependency.blocker.repository.to_ascii_lowercase(),
                    dependency.blocker.number,
                )),
            },
            BlockerScope::External => {
                (IssueState::parse(&dependency.blocker.state) != IssueState::Closed).then(|| {
                    UnsatisfiedBlocker::Opaque(
                        dependency.blocker.repository.to_ascii_lowercase(),
                        dependency.blocker.number,
                    )
                })
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn analyze_ready<'a>(
    replica: &'a LocalReplica,
    scope: ExecutionScope<'_>,
) -> ReadyAnalysis<'a> {
    OperationalGraph::prepare(replica).analyze_ready(scope)
}

fn strongly_connected_components(nodes: &[u64], edges: &BTreeSet<(u64, u64)>) -> Vec<Vec<u64>> {
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

fn cyclic_issue_numbers(components: &[Vec<u64>], edges: &BTreeSet<(u64, u64)>) -> BTreeSet<u64> {
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
