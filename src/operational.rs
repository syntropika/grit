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

pub(crate) fn analyze_ready<'a>(
    replica: &'a LocalReplica,
    scope: ExecutionScope<'_>,
) -> ReadyAnalysis<'a> {
    let issue_states: BTreeMap<_, _> = replica
        .issues
        .iter()
        .map(|issue| (issue.number, issue.state.as_str()))
        .collect();
    let mut ready = Vec::new();

    for issue in replica
        .issues
        .iter()
        .filter(|issue| issue.state.eq_ignore_ascii_case("open"))
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
                    .is_some_and(|state| state.eq_ignore_ascii_case("closed")),
                BlockerScope::External => dependency.blocker.state.eq_ignore_ascii_case("closed"),
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
        .filter(|state| state.eq_ignore_ascii_case("open"))
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
