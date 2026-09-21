use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::{
    model::Issue,
    operational::{BlockerResolution, ExecutionScope, OperationalGraph, PreparedRepository},
    priority::PriorityState,
    working_graph::{PendingProvenance, WorkingGraph},
};

pub(crate) struct StructuralPlan<'a> {
    pub(crate) parallel_now: Vec<PlanIssue<'a>>,
    pub(crate) dependency_layers: DependencyLayers<'a>,
}

#[derive(Serialize)]
pub(crate) struct PlanIssue<'a> {
    #[serde(flatten)]
    provenance: PendingProvenance,
    pub(crate) number: u64,
    pub(crate) url: &'a str,
    pub(crate) title: &'a str,
    pub(crate) ready_now: bool,
    pub(crate) assigned: bool,
    pub(crate) execution_scope_eligible: bool,
    pub(crate) executable: bool,
    pub(crate) priority: PriorityState,
    pub(crate) assignees: Vec<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct DependencyLayers<'a> {
    interpretation: &'static str,
    layers: Vec<DependencyLayer<'a>>,
    unresolved: Vec<UnresolvedIssue<'a>>,
}

impl DependencyLayers<'_> {
    pub(crate) fn human_lines(&self) -> Vec<String> {
        let mut lines: Vec<_> = self
            .layers
            .iter()
            .map(|layer| {
                let issues = layer
                    .issues
                    .iter()
                    .map(|issue| {
                        let eligibility = if issue.executable {
                            "executable now"
                        } else if issue.execution_scope_eligible {
                            "execution-scope eligible after blockers"
                        } else if issue.assigned {
                            "assigned outside execution scope"
                        } else {
                            "outside execution scope"
                        };
                        format!("#{} ({eligibility})", issue.number)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("layer {}: {issues}", layer.index)
            })
            .collect();
        if !self.unresolved.is_empty() {
            let issues = self
                .unresolved
                .iter()
                .map(|entry| format!("#{}", entry.issue.number))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("unresolved: {issues}"));
        }
        lines
    }
}

#[derive(Serialize)]
struct DependencyLayer<'a> {
    index: usize,
    issues: Vec<PlanIssue<'a>>,
}

#[derive(Serialize)]
struct UnresolvedIssue<'a> {
    issue: PlanIssue<'a>,
    reasons: Vec<UnresolvedReason>,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum UnresolvedReason {
    Cycle,
    OpaqueExternalBlocker,
    UnknownInternalBlocker,
    DependsOnUnresolved,
}

pub(crate) fn analyze<'a>(
    prepared: &PreparedRepository<'a>,
    scope: ExecutionScope<'_>,
) -> StructuralPlan<'a> {
    let graph = prepared.graph();
    let working = prepared.working();
    let ready = graph.analyze_ready(scope);
    let parallel_now = ready
        .executable
        .iter()
        .map(|issue| plan_issue(working, graph, issue, scope))
        .collect();
    StructuralPlan {
        parallel_now,
        dependency_layers: dependency_layers(working, graph, scope),
    }
}

fn dependency_layers<'a>(
    working: &WorkingGraph<'_>,
    graph: &OperationalGraph<'a>,
    scope: ExecutionScope<'_>,
) -> DependencyLayers<'a> {
    let mut unresolved = initial_unresolved(graph);
    propagate_unresolved(graph, &mut unresolved);

    let finite_numbers: BTreeSet<_> = graph
        .open_numbers()
        .iter()
        .copied()
        .filter(|number| !unresolved.contains_key(number))
        .collect();
    let mut blocker_counts: BTreeMap<_, _> = finite_numbers
        .iter()
        .map(|number| (*number, 0_usize))
        .collect();
    let mut dependents = BTreeMap::<u64, Vec<u64>>::new();
    for (blocked, blocker) in graph.internal_open_edges() {
        if finite_numbers.contains(blocked) && finite_numbers.contains(blocker) {
            *blocker_counts
                .get_mut(blocked)
                .expect("finite open Issue has a blocker counter") += 1;
            dependents.entry(*blocker).or_default().push(*blocked);
        }
    }

    let mut ready: BTreeSet<_> = blocker_counts
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(number, _)| *number)
        .collect();
    let mut finite_layers = BTreeMap::<u64, usize>::new();
    let mut latest_blocker_layer = BTreeMap::<u64, usize>::new();
    while let Some(number) = ready.pop_first() {
        let layer = latest_blocker_layer.get(&number).copied().unwrap_or(0);
        finite_layers.insert(number, layer);
        for dependent in dependents.get(&number).into_iter().flatten() {
            let latest = latest_blocker_layer.entry(*dependent).or_default();
            *latest = (*latest).max(layer + 1);
            let count = blocker_counts
                .get_mut(dependent)
                .expect("finite dependent has a blocker counter");
            *count -= 1;
            if *count == 0 {
                ready.insert(*dependent);
            }
        }
    }

    for number in finite_numbers {
        if !finite_layers.contains_key(&number) {
            unresolved
                .entry(number)
                .or_default()
                .insert(UnresolvedReason::DependsOnUnresolved);
        }
    }

    let mut by_layer = BTreeMap::<usize, Vec<PlanIssue<'a>>>::new();
    for (number, layer) in finite_layers {
        let issue = graph
            .issue(number)
            .expect("an open Operational Issue exists in the graph");
        by_layer
            .entry(layer)
            .or_default()
            .push(plan_issue(working, graph, issue, scope));
    }
    let layers = by_layer
        .into_iter()
        .map(|(index, issues)| DependencyLayer { index, issues })
        .collect();
    let unresolved = unresolved
        .into_iter()
        .filter_map(|(number, reasons)| {
            graph.issue(number).map(|issue| UnresolvedIssue {
                issue: plan_issue(working, graph, issue, scope),
                reasons: reasons.into_iter().collect(),
            })
        })
        .collect();

    DependencyLayers {
        interpretation: "counterfactual_dependency_layers",
        layers,
        unresolved,
    }
}

fn initial_unresolved(graph: &OperationalGraph<'_>) -> BTreeMap<u64, BTreeSet<UnresolvedReason>> {
    let mut unresolved = BTreeMap::<u64, BTreeSet<UnresolvedReason>>::new();
    for number in graph.open_numbers() {
        if graph.cyclic_numbers().contains(number) {
            unresolved
                .entry(*number)
                .or_default()
                .insert(UnresolvedReason::Cycle);
        }
        for dependency in graph.dependencies_for(*number) {
            match graph.blocker_resolution(dependency) {
                BlockerResolution::ExternalOpen | BlockerResolution::ExternalUnknown => {
                    unresolved
                        .entry(*number)
                        .or_default()
                        .insert(UnresolvedReason::OpaqueExternalBlocker);
                }
                BlockerResolution::InternalUnknown => {
                    unresolved
                        .entry(*number)
                        .or_default()
                        .insert(UnresolvedReason::UnknownInternalBlocker);
                }
                BlockerResolution::Satisfied | BlockerResolution::InternalOpen(_) => {}
            }
        }
    }
    unresolved
}

fn propagate_unresolved(
    graph: &OperationalGraph<'_>,
    unresolved: &mut BTreeMap<u64, BTreeSet<UnresolvedReason>>,
) {
    let mut queue: VecDeque<_> = unresolved.keys().copied().collect();
    while let Some(number) = queue.pop_front() {
        for dependent in graph.dependents_for(number) {
            if unresolved.contains_key(dependent) {
                continue;
            }
            let reasons = unresolved.entry(*dependent).or_default();
            if reasons.insert(UnresolvedReason::DependsOnUnresolved) {
                queue.push_back(*dependent);
            }
        }
    }
}

fn plan_issue<'a>(
    working: &WorkingGraph<'_>,
    graph: &OperationalGraph<'a>,
    issue: &'a Issue,
    scope: ExecutionScope<'_>,
) -> PlanIssue<'a> {
    let ready_now = graph.is_ready(issue.number);
    let execution_scope_eligible = scope.contains(issue);
    PlanIssue {
        provenance: working.provenance_for_issue(issue.number),
        number: issue.number,
        url: &issue.url,
        title: &issue.title,
        ready_now,
        assigned: !issue.assignees.is_empty(),
        execution_scope_eligible,
        executable: ready_now && execution_scope_eligible,
        priority: working.priority(issue),
        assignees: issue
            .assignees
            .iter()
            .map(|actor| actor.login.as_str())
            .collect(),
    }
}
