use std::collections::{BTreeMap, BTreeSet};

use super::{ExecutionScope, IssueState, OperationalGraph};
use crate::model::{BlockerScope, Dependency, Issue};

enum BlockerResolution {
    Satisfied,
    Internal(u64),
    OpaqueExternal,
}

pub(crate) struct Completion {
    issue_number: u64,
    newly_ready: Vec<u64>,
    depth: usize,
}

pub(crate) struct OneStepAnalysis<'issues> {
    executable: BTreeMap<u64, &'issues Issue>,
    unlocks_by_blocker: BTreeMap<u64, Vec<u64>>,
}

impl<'issues> OneStepAnalysis<'issues> {
    pub(crate) fn executable(&self) -> impl Iterator<Item = &'issues Issue> + '_ {
        self.executable.values().copied()
    }

    pub(crate) fn unlocks(&self) -> impl ExactSizeIterator<Item = (u64, &[u64])> + '_ {
        self.unlocks_by_blocker
            .iter()
            .map(|(blocker, unlocks)| (*blocker, unlocks.as_slice()))
    }

    pub(crate) fn unlocks_for(&self, blocker: u64) -> &[u64] {
        self.unlocks_by_blocker
            .get(&blocker)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

impl Completion {
    pub(crate) fn newly_ready(&self) -> &[u64] {
        &self.newly_ready
    }
}

#[derive(Clone)]
pub(crate) struct RolloutState<'graph, 'issues, 'scope> {
    graph: &'graph OperationalGraph<'issues>,
    scope: ExecutionScope<'scope>,
    ready: BTreeSet<u64>,
    completed: BTreeSet<u64>,
}

impl<'a> OperationalGraph<'a> {
    pub(crate) fn rollout_state<'graph, 'scope>(
        &'graph self,
        scope: ExecutionScope<'scope>,
    ) -> RolloutState<'graph, 'a, 'scope> {
        RolloutState::new(self, scope)
    }

    fn is_ready_after(&self, number: u64, completed: &BTreeSet<u64>) -> bool {
        if self.issue_state(number) != Some(IssueState::Open)
            || self.cyclic_numbers.contains(&number)
            || completed.contains(&number)
        {
            return false;
        }
        self.dependencies_for(number).iter().all(|dependency| {
            matches!(
                self.blocker_resolution(dependency, completed),
                BlockerResolution::Satisfied
            )
        })
    }

    fn blocker_resolution(
        &self,
        dependency: &Dependency,
        completed: &BTreeSet<u64>,
    ) -> BlockerResolution {
        match dependency.blocker.scope {
            BlockerScope::Internal => {
                let blocker = dependency.blocker.number;
                if completed.contains(&blocker)
                    || self.issue_state(blocker) == Some(IssueState::Closed)
                {
                    BlockerResolution::Satisfied
                } else {
                    BlockerResolution::Internal(blocker)
                }
            }
            BlockerScope::External => {
                if IssueState::parse(&dependency.blocker.state) == IssueState::Closed {
                    BlockerResolution::Satisfied
                } else {
                    BlockerResolution::OpaqueExternal
                }
            }
        }
    }
}

impl<'graph, 'issues, 'scope> RolloutState<'graph, 'issues, 'scope> {
    fn new(graph: &'graph OperationalGraph<'issues>, scope: ExecutionScope<'scope>) -> Self {
        let completed = BTreeSet::new();
        let ready = graph
            .open_numbers
            .iter()
            .copied()
            .filter(|number| graph.is_ready_after(*number, &completed))
            .collect();
        Self {
            graph,
            scope,
            ready,
            completed,
        }
    }

    pub(crate) fn executable(&self) -> Vec<&'issues Issue> {
        self.ready
            .iter()
            .filter_map(|number| self.graph.issue(*number))
            .filter(|issue| self.scope.contains(issue))
            .collect()
    }

    pub(crate) fn one_step_analysis(&self) -> OneStepAnalysis<'issues> {
        let executable: BTreeMap<_, _> = self
            .executable()
            .into_iter()
            .map(|issue| (issue.number, issue))
            .collect();
        let mut unlocks = BTreeMap::<u64, Vec<u64>>::new();
        for dependent in &self.graph.open_numbers {
            if self.ready.contains(dependent)
                || self.completed.contains(dependent)
                || self.graph.cyclic_numbers.contains(dependent)
            {
                continue;
            }
            let mut sole_blocker = None;
            let mut feasible = true;
            for dependency in self.graph.dependencies_for(*dependent) {
                match self.graph.blocker_resolution(dependency, &self.completed) {
                    BlockerResolution::Satisfied => {}
                    BlockerResolution::OpaqueExternal => {
                        feasible = false;
                        break;
                    }
                    BlockerResolution::Internal(blocker) => match sole_blocker {
                        None => sole_blocker = Some(blocker),
                        Some(existing) if existing == blocker => {}
                        Some(_) => {
                            feasible = false;
                            break;
                        }
                    },
                }
            }
            if feasible
                && let Some(blocker) = sole_blocker
                && executable.contains_key(&blocker)
            {
                unlocks.entry(blocker).or_default().push(*dependent);
            }
        }
        OneStepAnalysis {
            executable,
            unlocks_by_blocker: unlocks,
        }
    }

    pub(crate) fn feasible_prerequisite_closure(
        &self,
        target: u64,
        maximum_size: usize,
    ) -> Option<BTreeSet<u64>> {
        self.graph.issue(target)?;
        if self.completed.contains(&target)
            || self.graph.issue_state(target) != Some(IssueState::Open)
            || self.graph.cyclic_numbers.contains(&target)
        {
            return None;
        }

        let mut required = BTreeSet::new();
        self.collect_open_prerequisites(target, maximum_size, &mut required)?;
        Some(required)
    }

    pub(crate) fn graph(&self) -> &'graph OperationalGraph<'issues> {
        self.graph
    }

    pub(crate) fn is_ready(&self, issue_number: u64) -> bool {
        self.ready.contains(&issue_number)
    }

    pub(crate) fn is_completed(&self, issue_number: u64) -> bool {
        self.completed.contains(&issue_number)
    }

    pub(crate) fn complete(&mut self, issue_number: u64) -> Option<Completion> {
        if !self.is_executable(issue_number) {
            return None;
        }
        self.ready.remove(&issue_number);
        self.completed.insert(issue_number);
        let newly_ready: Vec<_> = self
            .graph
            .dependents_by_blocker
            .get(&issue_number)
            .into_iter()
            .flatten()
            .copied()
            .filter(|number| !self.ready.contains(number))
            .filter(|number| self.graph.is_ready_after(*number, &self.completed))
            .collect();
        self.ready.extend(newly_ready.iter().copied());
        Some(Completion {
            issue_number,
            newly_ready,
            depth: self.completed.len(),
        })
    }

    pub(crate) fn undo(&mut self, completion: Completion) {
        assert_eq!(
            self.completed.len(),
            completion.depth,
            "rollout completions must be undone in LIFO order"
        );
        for number in completion.newly_ready {
            self.ready.remove(&number);
        }
        self.completed.remove(&completion.issue_number);
        self.ready.insert(completion.issue_number);
    }

    fn is_executable(&self, issue_number: u64) -> bool {
        self.ready.contains(&issue_number)
            && self
                .graph
                .issue(issue_number)
                .is_some_and(|issue| self.scope.contains(issue))
    }

    fn collect_open_prerequisites(
        &self,
        issue_number: u64,
        maximum_size: usize,
        required: &mut BTreeSet<u64>,
    ) -> Option<()> {
        for dependency in self.graph.dependencies_for(issue_number) {
            match self.graph.blocker_resolution(dependency, &self.completed) {
                BlockerResolution::Satisfied => {}
                BlockerResolution::OpaqueExternal => return None,
                BlockerResolution::Internal(blocker) => {
                    let blocker_issue = self.graph.issue(blocker)?;
                    if self.graph.issue_state(blocker) != Some(IssueState::Open)
                        || self.graph.cyclic_numbers.contains(&blocker)
                        || !self.scope.contains(blocker_issue)
                    {
                        return None;
                    }
                    if required.insert(blocker) {
                        if required.len() > maximum_size {
                            return None;
                        }
                        self.collect_open_prerequisites(blocker, maximum_size, required)?;
                    }
                }
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BlockerIdentity, Dependency, IssueIdentity, LocalReplica, ReplicaError};

    #[test]
    fn rollouts_restore_nested_and_fanout_frontiers_in_lifo_order() -> Result<(), ReplicaError> {
        let replica = rollout_fixture()?;
        let graph = OperationalGraph::prepare(&replica);
        let mut state = graph.rollout_state(ExecutionScope::Available);

        assert_eq!(executable_numbers(&state), vec![1, 2]);
        assert_eq!(state.one_step_analysis().unlocks().len(), 0);
        let first = state.complete(1).expect("Issue #1 is Executable");
        assert!(first.newly_ready().is_empty());
        assert_eq!(executable_numbers(&state), vec![2]);
        assert_eq!(state.one_step_analysis().unlocks_for(2), [10].as_slice());
        let second = state.complete(2).expect("Issue #2 is Executable");
        assert_eq!(second.newly_ready(), &[10]);
        assert_eq!(executable_numbers(&state), vec![10]);
        assert_eq!(
            state.one_step_analysis().unlocks_for(10),
            [11, 12].as_slice()
        );
        let third = state.complete(10).expect("Issue #10 is Executable");
        assert_eq!(third.newly_ready(), &[11, 12]);
        assert_eq!(executable_numbers(&state), vec![11, 12]);

        state.undo(third);
        assert_eq!(executable_numbers(&state), vec![10]);
        state.undo(second);
        assert_eq!(executable_numbers(&state), vec![2]);
        state.undo(first);
        assert_eq!(executable_numbers(&state), vec![1, 2]);
        Ok(())
    }

    #[test]
    #[should_panic(expected = "rollout completions must be undone in LIFO order")]
    fn rejects_out_of_order_completion_rollback() {
        let replica = rollout_fixture().expect("valid rollout fixture");
        let graph = OperationalGraph::prepare(&replica);
        let mut state = graph.rollout_state(ExecutionScope::Available);
        let first = state.complete(1).expect("Issue #1 is Executable");
        let _second = state.complete(2).expect("Issue #2 is Executable");

        state.undo(first);
    }

    fn executable_numbers(state: &RolloutState<'_, '_, '_>) -> Vec<u64> {
        state
            .executable()
            .iter()
            .map(|issue| issue.number)
            .collect()
    }

    fn rollout_fixture() -> Result<LocalReplica, ReplicaError> {
        LocalReplica::build(
            "acme/widgets".to_owned(),
            "2026-08-07T00:00:00Z".to_owned(),
            Vec::new(),
            [1, 2, 10, 11, 12].into_iter().map(issue).collect(),
            vec![
                dependency(10, 1),
                dependency(10, 2),
                dependency(11, 10),
                dependency(12, 10),
            ],
        )
    }

    fn issue(number: u64) -> Issue {
        Issue {
            id: number,
            node_id: format!("I_{number}"),
            number,
            url: format!("https://github.com/acme/widgets/issues/{number}"),
            title: format!("Issue {number}"),
            body: String::new(),
            state: "open".to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            created_at: "2026-08-01T00:00:00Z".to_owned(),
            updated_at: "2026-08-01T00:00:00Z".to_owned(),
            closed_at: None,
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
