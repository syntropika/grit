use std::collections::{BTreeMap, BTreeSet, VecDeque};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    model::{Issue, strip_operation_markers},
    operational::{BlockerResolution, ExecutionScope, OperationalGraph, ReadyAnalysis},
    priority::PriorityState,
};

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StructuralPlan {
    pub(crate) parallel_now: Vec<PlanIssue>,
    pub(crate) dependency_layers: DependencyLayers,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanIssue {
    pub(crate) number: u64,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) ready_now: bool,
    pub(crate) assigned: bool,
    pub(crate) execution_scope_eligible: bool,
    pub(crate) executable: bool,
    pub(crate) priority: PriorityState,
    pub(crate) assignees: Vec<String>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DependencyLayers {
    interpretation: String,
    layers: Vec<DependencyLayer>,
    unresolved: Vec<UnresolvedIssue>,
}

impl DependencyLayers {
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

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct DependencyLayer {
    index: usize,
    issues: Vec<PlanIssue>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct UnresolvedIssue {
    issue: PlanIssue,
    reasons: Vec<UnresolvedReason>,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum UnresolvedReason {
    Cycle,
    OpaqueExternalBlocker,
    UnknownInternalBlocker,
    DependsOnUnresolved,
}

pub(crate) fn analyze_with_ready(
    graph: &OperationalGraph<'_>,
    scope: ExecutionScope<'_>,
    ready: &ReadyAnalysis<'_>,
) -> StructuralPlan {
    let parallel_now = ready
        .executable
        .iter()
        .map(|issue| plan_issue(graph, issue, scope))
        .collect();
    StructuralPlan {
        parallel_now,
        dependency_layers: dependency_layers(graph, scope),
    }
}

fn dependency_layers(graph: &OperationalGraph<'_>, scope: ExecutionScope<'_>) -> DependencyLayers {
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

    let mut by_layer = BTreeMap::<usize, Vec<PlanIssue>>::new();
    for (number, layer) in finite_layers {
        let issue = graph
            .issue(number)
            .expect("an open Operational Issue exists in the graph");
        by_layer
            .entry(layer)
            .or_default()
            .push(plan_issue(graph, issue, scope));
    }
    let layers = by_layer
        .into_iter()
        .map(|(index, issues)| DependencyLayer { index, issues })
        .collect();
    let unresolved = unresolved
        .into_iter()
        .filter_map(|(number, reasons)| {
            graph.issue(number).map(|issue| UnresolvedIssue {
                issue: plan_issue(graph, issue, scope),
                reasons: reasons.into_iter().collect(),
            })
        })
        .collect();

    DependencyLayers {
        interpretation: "counterfactual_dependency_layers".to_owned(),
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

fn plan_issue(graph: &OperationalGraph<'_>, issue: &Issue, scope: ExecutionScope<'_>) -> PlanIssue {
    let ready_now = graph.is_ready(issue.number);
    let execution_scope_eligible = scope.contains(issue);
    PlanIssue {
        number: issue.number,
        url: issue.url.clone(),
        title: strip_operation_markers(&issue.title),
        ready_now,
        assigned: !issue.assignees.is_empty(),
        execution_scope_eligible,
        executable: ready_now && execution_scope_eligible,
        priority: PriorityState::from_issue_labels(&issue.labels),
        assignees: issue
            .assignees
            .iter()
            .map(|actor| actor.login.clone())
            .collect(),
    }
}
