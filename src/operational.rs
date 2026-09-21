use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{BlockerScope, Dependency, Issue, LocalReplica};

#[derive(Clone, Copy)]
pub(crate) enum ExecutionScope<'a> {
    Available,
    Assignee(&'a str),
}

pub(crate) struct ReadyAnalysis<'a> {
    pub(crate) operational_issue_count: usize,
    pub(crate) ready_count: usize,
    pub(crate) assigned_ready_count: usize,
    pub(crate) blocked_count: usize,
    pub(crate) ready: Vec<&'a Issue>,
    pub(crate) executable: Vec<&'a Issue>,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
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
    replica: &'a LocalReplica,
    issue_states: BTreeMap<u64, IssueState>,
    dependencies_by_blocked: BTreeMap<u64, Vec<&'a Dependency>>,
}

impl<'a> OperationalGraph<'a> {
    pub(crate) fn prepare(replica: &'a LocalReplica) -> Self {
        let issue_states = replica
            .issues
            .iter()
            .map(|issue| (issue.number, IssueState::parse(&issue.state)))
            .collect();
        let mut dependencies_by_blocked = BTreeMap::<u64, Vec<&Dependency>>::new();
        for dependency in &replica.dependencies {
            if dependency
                .blocked
                .repository
                .eq_ignore_ascii_case(&replica.repository)
            {
                dependencies_by_blocked
                    .entry(dependency.blocked.number)
                    .or_default()
                    .push(dependency);
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
        Self {
            replica,
            issue_states,
            dependencies_by_blocked,
        }
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

    pub(crate) fn analyze_ready(&self, scope: ExecutionScope<'_>) -> ReadyAnalysis<'a> {
        let mut ready = Vec::new();

        for issue in self
            .replica
            .issues
            .iter()
            .filter(|issue| self.issue_state(issue.number) == Some(IssueState::Open))
        {
            let blockers_satisfied = self
                .dependencies_for(issue.number)
                .iter()
                .all(|dependency| match dependency.blocker.scope {
                    BlockerScope::Internal => self
                        .issue_state(dependency.blocker.number)
                        .is_some_and(|state| state == IssueState::Closed),
                    BlockerScope::External => {
                        IssueState::parse(&dependency.blocker.state) == IssueState::Closed
                    }
                });
            if blockers_satisfied {
                ready.push(issue);
            }
        }

        let assigned_ready_count = ready
            .iter()
            .filter(|issue| !issue.assignees.is_empty())
            .count();
        ready.sort_by_key(|issue| issue.number);
        let mut executable: Vec<_> = ready
            .iter()
            .copied()
            .filter(|issue| match scope {
                ExecutionScope::Available => issue.assignees.is_empty(),
                ExecutionScope::Assignee(assignee) => issue
                    .assignees
                    .iter()
                    .any(|actor| actor.login.eq_ignore_ascii_case(assignee)),
            })
            .collect();
        executable.sort_by_key(|issue| issue.number);

        let operational_issue_count = self
            .issue_states
            .values()
            .filter(|state| **state == IssueState::Open)
            .count();
        let ready_count = ready.len();
        ReadyAnalysis {
            operational_issue_count,
            ready_count,
            assigned_ready_count,
            blocked_count: operational_issue_count - ready_count,
            ready,
            executable,
        }
    }
}

pub(crate) fn analyze_ready<'a>(
    replica: &'a LocalReplica,
    scope: ExecutionScope<'_>,
) -> ReadyAnalysis<'a> {
    OperationalGraph::prepare(replica).analyze_ready(scope)
}
