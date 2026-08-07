use std::collections::BTreeMap;

use crate::model::{BlockerScope, Issue, LocalReplica};

pub(crate) enum ExecutionScope<'a> {
    Available,
    Assignee(&'a str),
}

pub(crate) struct ReadyAnalysis<'a> {
    pub(crate) operational_issue_count: usize,
    pub(crate) ready_count: usize,
    pub(crate) assigned_ready_count: usize,
    pub(crate) blocked_count: usize,
    pub(crate) executable: Vec<&'a Issue>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum IssueState {
    Open,
    Closed,
    Unknown,
}

impl IssueState {
    fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("open") {
            Self::Open
        } else if value.eq_ignore_ascii_case("closed") {
            Self::Closed
        } else {
            Self::Unknown
        }
    }
}

pub(crate) fn analyze_ready<'a>(
    replica: &'a LocalReplica,
    scope: ExecutionScope<'_>,
) -> ReadyAnalysis<'a> {
    let issue_states: BTreeMap<_, _> = replica
        .issues
        .iter()
        .map(|issue| (issue.number, IssueState::parse(&issue.state)))
        .collect();
    let mut ready = Vec::new();

    for issue in replica
        .issues
        .iter()
        .filter(|issue| IssueState::parse(&issue.state) == IssueState::Open)
    {
        let blockers_satisfied = replica
            .dependencies
            .iter()
            .filter(|dependency| {
                dependency.blocked.number == issue.number
                    && dependency
                        .blocked
                        .repository
                        .eq_ignore_ascii_case(&replica.repository)
            })
            .all(|dependency| match dependency.blocker.scope {
                BlockerScope::Internal => issue_states
                    .get(&dependency.blocker.number)
                    .is_some_and(|state| *state == IssueState::Closed),
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

    let operational_issue_count = issue_states
        .values()
        .filter(|state| **state == IssueState::Open)
        .count();
    let ready_count = ready.len();
    ReadyAnalysis {
        operational_issue_count,
        ready_count,
        assigned_ready_count,
        blocked_count: operational_issue_count - ready_count,
        executable,
    }
}
