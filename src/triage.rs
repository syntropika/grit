use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

mod scc;

use crate::{
    model::{BlockerScope, Dependency, Issue, LocalReplica},
    operational::{ExecutionScope, IssueState, OperationalGraph},
    priority::{DeclaredPriority, PriorityState},
};

#[derive(Serialize)]
pub(crate) struct TriageReport {
    diagnostics: Vec<TriageDiagnostic>,
    summary: TriageSummary,
}

impl TriageReport {
    pub(crate) fn human_lines(&self) -> Vec<String> {
        self.diagnostics
            .iter()
            .map(TriageDiagnostic::human_line)
            .collect()
    }
}

#[derive(Serialize)]
struct TriageDiagnostic {
    code: DiagnosticCode,
    subjects: Vec<TriageIssue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blockers: Vec<TriageBlocker>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
}

impl TriageDiagnostic {
    fn human_line(&self) -> String {
        let subjects = self
            .subjects
            .iter()
            .map(|subject| subject.key.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let blockers = self
            .blockers
            .iter()
            .map(|blocker| blocker.key.as_str())
            .collect::<Vec<_>>();
        let states = self
            .subjects
            .iter()
            .map(|subject| {
                format!(
                    "{}: readiness={}, availability={}, in_execution_scope={}",
                    subject.key,
                    subject.readiness.as_str(),
                    subject.availability.as_str(),
                    subject.in_execution_scope
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        if blockers.is_empty() {
            format!("[{}] {subjects} [{states}]", self.code.as_str())
        } else {
            format!(
                "[{}] {subjects} [{states}] (blockers: {})",
                self.code.as_str(),
                blockers.join(", ")
            )
        }
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum DiagnosticCode {
    BlockedP0,
    ExternalBlockerOpen,
    ExternalBlockerUnknown,
    DependencyCycle,
    AssignedReady,
    PriorityConflict,
}

impl DiagnosticCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::BlockedP0 => "blocked_p0",
            Self::ExternalBlockerOpen => "external_blocker_open",
            Self::ExternalBlockerUnknown => "external_blocker_unknown",
            Self::DependencyCycle => "dependency_cycle",
            Self::AssignedReady => "assigned_ready",
            Self::PriorityConflict => "priority_conflict",
        }
    }
}

#[derive(Serialize)]
struct TriageIssue {
    key: String,
    number: u64,
    url: String,
    title: String,
    readiness: Readiness,
    availability: Availability,
    in_execution_scope: bool,
    assignees: Vec<String>,
    priority: PriorityState,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Readiness {
    Ready,
    Blocked,
}

impl Readiness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Availability {
    Available,
    Assigned,
}

impl Availability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
        }
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct TriageBlocker {
    key: String,
    scope: BlockerScope,
    state: IssueState,
}

#[derive(Serialize)]
struct TriageSummary {
    diagnostic_count: usize,
    blocked_p0_count: usize,
    external_blocker_open_count: usize,
    external_blocker_unknown_count: usize,
    dependency_cycle_count: usize,
    assigned_ready_count: usize,
    priority_conflict_count: usize,
}

pub(crate) fn analyze(replica: &LocalReplica, scope: ExecutionScope<'_>) -> TriageReport {
    let graph = OperationalGraph::prepare(replica);
    let ready_analysis = graph.analyze_ready(scope);
    let ready_numbers: BTreeSet<_> = ready_analysis
        .ready
        .iter()
        .map(|issue| issue.number)
        .collect();
    let executable_numbers: BTreeSet<_> = ready_analysis
        .executable
        .iter()
        .map(|issue| issue.number)
        .collect();
    let issues: BTreeMap<_, _> = replica
        .issues
        .iter()
        .map(|issue| (issue.number, issue))
        .collect();
    let open_issues: Vec<_> = replica
        .issues
        .iter()
        .filter(|issue| graph.issue_state(issue.number) == Some(IssueState::Open))
        .collect();
    let mut diagnostics = Vec::new();

    for issue in &open_issues {
        let priority = PriorityState::from_issue_labels(&issue.labels);
        if matches!(
            priority,
            PriorityState::Declared {
                value: DeclaredPriority::P0
            }
        ) && !ready_numbers.contains(&issue.number)
        {
            diagnostics.push(TriageDiagnostic {
                code: DiagnosticCode::BlockedP0,
                subjects: vec![triage_issue(
                    &replica.repository,
                    issue,
                    &ready_numbers,
                    &executable_numbers,
                )],
                blockers: unsatisfied_blockers(&graph, issue),
                labels: Vec::new(),
            });
        }
    }

    for issue in &open_issues {
        for (code, state) in [
            (DiagnosticCode::ExternalBlockerOpen, IssueState::Open),
            (DiagnosticCode::ExternalBlockerUnknown, IssueState::Unknown),
        ] {
            let blockers: Vec<_> = graph
                .dependencies_for(issue.number)
                .iter()
                .filter(|dependency| matches!(dependency.blocker.scope, BlockerScope::External))
                .filter(|dependency| IssueState::parse(&dependency.blocker.state) == state)
                .map(|dependency| blocker_output(dependency, state))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if !blockers.is_empty() {
                diagnostics.push(TriageDiagnostic {
                    code,
                    subjects: vec![triage_issue(
                        &replica.repository,
                        issue,
                        &ready_numbers,
                        &executable_numbers,
                    )],
                    blockers,
                    labels: Vec::new(),
                });
            }
        }
    }

    for component in scc::cyclic_components(&graph, &issues) {
        diagnostics.push(TriageDiagnostic {
            code: DiagnosticCode::DependencyCycle,
            subjects: component
                .into_iter()
                .filter_map(|number| issues.get(&number).copied())
                .map(|issue| {
                    triage_issue(
                        &replica.repository,
                        issue,
                        &ready_numbers,
                        &executable_numbers,
                    )
                })
                .collect(),
            blockers: Vec::new(),
            labels: Vec::new(),
        });
    }

    for issue in &open_issues {
        if ready_numbers.contains(&issue.number) && !issue.assignees.is_empty() {
            diagnostics.push(TriageDiagnostic {
                code: DiagnosticCode::AssignedReady,
                subjects: vec![triage_issue(
                    &replica.repository,
                    issue,
                    &ready_numbers,
                    &executable_numbers,
                )],
                blockers: Vec::new(),
                labels: Vec::new(),
            });
        }
    }

    for issue in &open_issues {
        let priority = PriorityState::from_issue_labels(&issue.labels);
        if let Some(labels) = priority.conflict_labels() {
            diagnostics.push(TriageDiagnostic {
                code: DiagnosticCode::PriorityConflict,
                subjects: vec![triage_issue(
                    &replica.repository,
                    issue,
                    &ready_numbers,
                    &executable_numbers,
                )],
                blockers: Vec::new(),
                labels: labels.to_vec(),
            });
        }
    }

    diagnostics.sort_by(|left, right| {
        left.code.cmp(&right.code).then_with(|| {
            left.subjects
                .first()
                .map(|issue| issue.key.as_str())
                .cmp(&right.subjects.first().map(|issue| issue.key.as_str()))
        })
    });
    let summary = TriageSummary {
        diagnostic_count: diagnostics.len(),
        blocked_p0_count: count(&diagnostics, DiagnosticCode::BlockedP0),
        external_blocker_open_count: count(&diagnostics, DiagnosticCode::ExternalBlockerOpen),
        external_blocker_unknown_count: count(&diagnostics, DiagnosticCode::ExternalBlockerUnknown),
        dependency_cycle_count: count(&diagnostics, DiagnosticCode::DependencyCycle),
        assigned_ready_count: count(&diagnostics, DiagnosticCode::AssignedReady),
        priority_conflict_count: count(&diagnostics, DiagnosticCode::PriorityConflict),
    };
    TriageReport {
        diagnostics,
        summary,
    }
}

fn triage_issue(
    repository: &str,
    issue: &Issue,
    ready_numbers: &BTreeSet<u64>,
    executable_numbers: &BTreeSet<u64>,
) -> TriageIssue {
    let ready = ready_numbers.contains(&issue.number);
    let available = issue.assignees.is_empty();
    TriageIssue {
        key: format!("{repository}#{}", issue.number),
        number: issue.number,
        url: issue.url.clone(),
        title: issue.title.clone(),
        readiness: if ready {
            Readiness::Ready
        } else {
            Readiness::Blocked
        },
        availability: if available {
            Availability::Available
        } else {
            Availability::Assigned
        },
        in_execution_scope: executable_numbers.contains(&issue.number),
        assignees: issue
            .assignees
            .iter()
            .map(|actor| actor.login.clone())
            .collect(),
        priority: PriorityState::from_issue_labels(&issue.labels),
    }
}

fn unsatisfied_blockers(graph: &OperationalGraph<'_>, issue: &Issue) -> Vec<TriageBlocker> {
    graph
        .dependencies_for(issue.number)
        .iter()
        .filter_map(|dependency| match dependency.blocker.scope {
            BlockerScope::Internal => {
                let state = graph
                    .issue_state(dependency.blocker.number)
                    .unwrap_or(IssueState::Unknown);
                (state != IssueState::Closed).then(|| blocker_output(dependency, state))
            }
            BlockerScope::External => {
                let state = IssueState::parse(&dependency.blocker.state);
                (state != IssueState::Closed).then(|| blocker_output(dependency, state))
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn blocker_output(dependency: &Dependency, state: IssueState) -> TriageBlocker {
    TriageBlocker {
        key: format!(
            "{}#{}",
            dependency.blocker.repository, dependency.blocker.number
        ),
        scope: dependency.blocker.scope,
        state,
    }
}

fn count(diagnostics: &[TriageDiagnostic], code: DiagnosticCode) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .count()
}
