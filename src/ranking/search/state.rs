use super::*;

#[derive(Clone)]
pub(super) struct PartialRollout<'a> {
    pub(super) steps: Vec<EvaluatedStep<'a>>,
    pub(super) unlocks: BTreeSet<u64>,
    pub(super) unlock_curve: Vec<usize>,
    pub(super) p0_curve: Vec<usize>,
}

impl<'a> PartialRollout<'a> {
    pub(super) fn apply(
        &mut self,
        step: EvaluatedStep<'a>,
        newly_ready: &[u64],
        graph: &crate::operational::OperationalGraph<'a>,
        working: &WorkingGraph<'_>,
    ) -> PartialCheckpoint {
        let steps_len = self.steps.len();
        let unlock_curve_len = self.unlock_curve.len();
        let p0_curve_len = self.p0_curve.len();
        let mut inserted_unlocks = Vec::new();
        self.steps.push(step);
        for number in newly_ready {
            if self.unlocks.insert(*number) {
                inserted_unlocks.push(*number);
            }
        }
        self.unlock_curve.push(self.unlocks.len());
        let unlocked_p0 = self
            .unlocks
            .iter()
            .filter_map(|number| graph.issue(*number))
            .filter(|issue| priority(working, issue) == PriorityComparison::P0)
            .count();
        self.p0_curve.push(unlocked_p0);
        PartialCheckpoint {
            steps_len,
            unlock_curve_len,
            p0_curve_len,
            inserted_unlocks,
        }
    }

    pub(super) fn undo(&mut self, checkpoint: PartialCheckpoint) {
        assert_eq!(
            self.steps.len(),
            checkpoint.steps_len + 1,
            "partial rollouts must be undone in LIFO order"
        );
        assert_eq!(self.unlock_curve.len(), checkpoint.unlock_curve_len + 1);
        assert_eq!(self.p0_curve.len(), checkpoint.p0_curve_len + 1);
        self.steps.truncate(checkpoint.steps_len);
        self.unlock_curve.truncate(checkpoint.unlock_curve_len);
        self.p0_curve.truncate(checkpoint.p0_curve_len);
        for number in checkpoint.inserted_unlocks {
            debug_assert!(self.unlocks.remove(&number));
        }
    }
}

pub(super) struct PartialCheckpoint {
    steps_len: usize,
    unlock_curve_len: usize,
    p0_curve_len: usize,
    inserted_unlocks: Vec<u64>,
}

#[derive(Clone, Default)]
pub(super) struct CausalCone {
    newly_ready: BTreeSet<u64>,
    co_blockers: BTreeSet<u64>,
    members: BTreeSet<u64>,
    pub(super) realized_continuations: usize,
}

impl CausalCone {
    pub(super) fn advance(
        &mut self,
        completed: u64,
        newly_ready: &[u64],
        graph: &crate::operational::OperationalGraph<'_>,
    ) {
        if self.members.contains(&completed) {
            self.realized_continuations += 1;
        }
        self.newly_ready.remove(&completed);
        self.co_blockers.remove(&completed);
        self.members.remove(&completed);
        for number in newly_ready {
            self.newly_ready.insert(*number);
            self.members.insert(*number);
        }
        for dependent in graph.dependents_for(completed) {
            self.members.insert(*dependent);
            for dependency in graph.dependencies_for(*dependent) {
                if matches!(dependency.blocker.scope, BlockerScope::Internal)
                    && graph.issue_state(dependency.blocker.number)
                        == Some(crate::operational::IssueState::Open)
                {
                    self.co_blockers.insert(dependency.blocker.number);
                    self.members.insert(dependency.blocker.number);
                }
            }
        }
    }

    pub(super) fn contains(&self, number: u64) -> bool {
        self.members.contains(&number)
    }

    pub(super) fn category(&self, number: u64) -> u8 {
        if self.newly_ready.contains(&number) {
            2
        } else if self.co_blockers.contains(&number) {
            1
        } else {
            0
        }
    }
}

#[derive(Clone)]
pub(super) struct SearchState<'graph, 'issues, 'scope> {
    pub(super) rollout: RolloutState<'graph, 'issues, 'scope>,
    pub(super) partial: PartialRollout<'issues>,
    pub(super) causal: CausalCone,
    pub(super) joint_plan: Option<PartialRollout<'issues>>,
    pub(super) upper: PotentialKey,
}

impl<'graph, 'issues, 'scope> SearchState<'graph, 'issues, 'scope> {
    pub(super) fn root(rollout: RolloutState<'graph, 'issues, 'scope>, horizon: u8) -> Self {
        Self {
            rollout,
            partial: PartialRollout {
                steps: Vec::with_capacity(horizon as usize),
                unlocks: BTreeSet::new(),
                unlock_curve: Vec::with_capacity(horizon as usize),
                p0_curve: Vec::with_capacity(horizon as usize),
            },
            causal: CausalCone::default(),
            joint_plan: None,
            upper: PotentialKey::default(),
        }
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct PotentialKey {
    pub(super) p0_curve: Vec<usize>,
    pub(super) unlock_count: usize,
    pub(super) priority_profile: [usize; 4],
    pub(super) unlock_curve: Vec<usize>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(super) struct StructuralKey {
    pub(super) count: usize,
    pub(super) priority_profile: [usize; 5],
}

#[derive(Clone)]
pub(super) struct ScoredStep<'a> {
    pub(super) step: EvaluatedStep<'a>,
    pub(super) immediate: PartialRollout<'a>,
    pub(super) structural: StructuralKey,
    pub(super) upper: PotentialKey,
    pub(super) joint_plan: Option<PartialRollout<'a>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Issue, LocalReplica};

    #[test]
    #[should_panic(expected = "partial rollouts must be undone in LIFO order")]
    fn rejects_out_of_order_partial_rollback() {
        let replica = LocalReplica {
            schema_version: "grit.local-replica/v1".to_owned(),
            repository: "acme/rollback".to_owned(),
            synced_at: "2026-08-07T00:00:00Z".to_owned(),
            input_hash: "fixture".to_owned(),
            repository_labels: Some(Vec::new()),
            sync: Default::default(),
            issues: vec![issue(1), issue(2)],
            dependencies: Vec::new(),
        };
        let outbox = super::super::tests::empty_outbox(&replica.repository);
        let working = WorkingGraph::project(&replica, &outbox).expect("Working graph");
        let graph = crate::operational::OperationalGraph::prepare(&replica);
        let mut rollout = graph.rollout_state(ExecutionScope::Available);
        let mut partial = SearchState::root(rollout.clone(), 3).partial;

        let first_completion = rollout.complete(1).expect("Issue #1 is Executable");
        let first = partial.apply(
            EvaluatedStep {
                issue: graph.issue(1).expect("Issue #1"),
                selection: StepSelection::Normal,
            },
            first_completion.newly_ready(),
            &graph,
            &working,
        );
        let second_completion = rollout.complete(2).expect("Issue #2 is Executable");
        let _second = partial.apply(
            EvaluatedStep {
                issue: graph.issue(2).expect("Issue #2"),
                selection: StepSelection::Normal,
            },
            second_completion.newly_ready(),
            &graph,
            &working,
        );

        partial.undo(first);
    }

    fn issue(number: u64) -> Issue {
        Issue {
            id: number,
            node_id: format!("I_{number}"),
            number,
            url: format!("https://github.com/acme/rollback/issues/{number}"),
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
}
