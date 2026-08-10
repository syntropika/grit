use std::{cmp::Ordering, collections::BTreeMap, collections::BTreeSet, num::NonZeroUsize};

use serde::Serialize;

use super::{
    CandidateData, CriticalRouteOutcome, EvaluatedCandidate, EvaluatedStep, StepSelection,
    decision::{PriorityProfile, RankingMode, StepPriority},
    pagerank::PageRank,
    priority,
};
use crate::{
    operational::{ExecutionScope, RolloutState},
    priority::PriorityComparison,
};

pub(super) struct SearchResult<'a> {
    pub(super) mode: RankingMode,
    pub(super) candidate_count: usize,
    pub(super) candidates: Vec<EvaluatedCandidate<'a>>,
    pub(super) truncated_by: Vec<SearchRestriction>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SearchRestriction {
    StateBudget,
}

impl SearchRestriction {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::StateBudget => "state_budget",
        }
    }
}

struct PartialRollout<'a> {
    steps: Vec<EvaluatedStep<'a>>,
    unlocks: BTreeSet<u64>,
    unlock_curve: Vec<usize>,
    p0_curve: Vec<usize>,
}

struct PartialCheckpoint {
    depth: usize,
    added_unlocks: Vec<u64>,
}

impl<'a> PartialRollout<'a> {
    fn apply(
        &mut self,
        step: EvaluatedStep<'a>,
        newly_ready: &[u64],
        graph: &crate::operational::OperationalGraph<'a>,
    ) -> PartialCheckpoint {
        self.steps.push(step);
        let mut added_unlocks = Vec::new();
        for number in newly_ready {
            if self.unlocks.insert(*number) {
                added_unlocks.push(*number);
            }
        }
        self.unlock_curve.push(self.unlocks.len());
        let unlocked_p0 = self
            .unlocks
            .iter()
            .filter_map(|number| graph.issue(*number))
            .filter(|issue| priority(issue) == PriorityComparison::P0)
            .count();
        self.p0_curve.push(unlocked_p0);
        PartialCheckpoint {
            depth: self.steps.len(),
            added_unlocks,
        }
    }

    fn undo(&mut self, checkpoint: PartialCheckpoint) {
        assert_eq!(
            self.steps.len(),
            checkpoint.depth,
            "partial rollout checkpoints must be undone in LIFO order"
        );
        self.p0_curve.pop();
        self.unlock_curve.pop();
        for number in checkpoint.added_unlocks {
            self.unlocks.remove(&number);
        }
        self.steps.pop();
    }
}

struct Search<'graph, 'issues, 'scope, 'pagerank> {
    state: RolloutState<'graph, 'issues, 'scope>,
    pagerank: Option<&'pagerank PageRank>,
    horizon: u8,
    state_budget: usize,
    expanded_states: usize,
    state_budget_exhausted: bool,
    p0_targets: Vec<u64>,
    best_by_first: BTreeMap<u64, EvaluatedCandidate<'issues>>,
}

pub(super) fn evaluate<'graph, 'issues, 'scope, 'pagerank>(
    graph: &'graph crate::operational::OperationalGraph<'issues>,
    scope: ExecutionScope<'scope>,
    pagerank: Option<&'pagerank PageRank>,
    horizon: u8,
    state_budget: usize,
) -> SearchResult<'issues> {
    let state = graph.rollout_state(scope);
    let p0_targets = graph
        .open_numbers()
        .iter()
        .copied()
        .filter(|number| {
            graph
                .issue(*number)
                .is_some_and(|issue| priority(issue) == PriorityComparison::P0)
        })
        .collect::<Vec<_>>();
    let first_frontier = frontier(&state, horizon as usize, &p0_targets);
    let initial_mode = first_frontier.mode;
    let candidate_count = first_frontier.steps.len();
    let first_steps_count_against_budget = horizon > 1;
    let mut search = Search {
        state,
        pagerank,
        horizon,
        state_budget,
        expanded_states: if first_steps_count_against_budget {
            candidate_count.min(state_budget)
        } else {
            0
        },
        state_budget_exhausted: first_steps_count_against_budget && candidate_count > state_budget,
        p0_targets,
        best_by_first: BTreeMap::new(),
    };
    for first_step in first_frontier.steps {
        let mut partial = PartialRollout {
            steps: Vec::with_capacity(horizon as usize),
            unlocks: BTreeSet::new(),
            unlock_curve: Vec::with_capacity(horizon as usize),
            p0_curve: Vec::with_capacity(horizon as usize),
        };
        search.explore(first_step, &mut partial, false);
    }
    SearchResult {
        mode: initial_mode,
        candidate_count,
        candidates: search.best_by_first.into_values().collect(),
        truncated_by: search
            .state_budget_exhausted
            .then_some(SearchRestriction::StateBudget)
            .into_iter()
            .collect(),
    }
}

impl<'graph, 'issues, 'scope, 'pagerank> Search<'graph, 'issues, 'scope, 'pagerank> {
    fn explore(
        &mut self,
        step: EvaluatedStep<'issues>,
        partial: &mut PartialRollout<'issues>,
        counts_against_budget: bool,
    ) {
        if counts_against_budget {
            if self.expanded_states >= self.state_budget {
                self.state_budget_exhausted = true;
                return;
            }
            self.expanded_states += 1;
        }
        let completion = self
            .state
            .complete(step.issue.number)
            .expect("search frontiers contain only Executable Issues");
        let checkpoint = partial.apply(step, completion.newly_ready(), self.state.graph());
        self.record(partial);

        if partial.steps.len() < self.horizon as usize && !self.state_budget_exhausted {
            let remaining_steps = self.horizon as usize - partial.steps.len();
            let next_frontier = frontier(&self.state, remaining_steps, &self.p0_targets);
            for next_step in next_frontier.steps {
                self.explore(next_step, partial, true);
                if self.state_budget_exhausted && self.expanded_states >= self.state_budget {
                    break;
                }
            }
        }

        partial.undo(checkpoint);
        self.state.undo(completion);
    }

    fn record(&mut self, partial: &PartialRollout<'issues>) {
        let candidate = snapshot(partial, self.state.graph(), self.pagerank, self.horizon);
        let first_number = candidate.data().issue.number;
        let replace = self
            .best_by_first
            .get(&first_number)
            .is_none_or(|current| compare_same_first(&candidate, current).is_gt());
        if replace {
            self.best_by_first.insert(first_number, candidate);
        }
    }
}

struct Frontier<'a> {
    mode: RankingMode,
    steps: Vec<EvaluatedStep<'a>>,
}

fn frontier<'issues>(
    state: &RolloutState<'_, 'issues, '_>,
    remaining_steps: usize,
    p0_targets: &[u64],
) -> Frontier<'issues> {
    let executable: BTreeMap<_, _> = state
        .executable()
        .into_iter()
        .map(|issue| (issue.number, issue))
        .collect();
    let executable_p0: Vec<_> = executable
        .values()
        .copied()
        .filter(|issue| priority(issue) == PriorityComparison::P0)
        .collect();
    if !executable_p0.is_empty() {
        return Frontier {
            mode: RankingMode::P0Ready,
            steps: executable_p0
                .into_iter()
                .map(|issue| EvaluatedStep {
                    issue,
                    selection: StepSelection::P0Ready,
                })
                .collect(),
        };
    }

    let mut route_by_step = BTreeMap::<u64, RouteMembership>::new();
    for target in p0_targets {
        let Some(closure) = state.feasible_prerequisite_closure(*target, remaining_steps) else {
            continue;
        };
        if closure.is_empty() {
            continue;
        }
        let distance = NonZeroUsize::new(closure.len()).expect("non-empty P0 closure");
        for step_number in closure {
            if executable.contains_key(&step_number) {
                route_by_step
                    .entry(step_number)
                    .and_modify(|membership| membership.include(distance))
                    .or_insert_with(|| RouteMembership::new(distance));
            }
        }
    }

    if !route_by_step.is_empty() {
        Frontier {
            mode: RankingMode::P0Route,
            steps: route_by_step
                .into_iter()
                .map(|(number, membership)| EvaluatedStep {
                    issue: executable
                        .get(&number)
                        .copied()
                        .expect("route membership contains only Executable Issues"),
                    selection: StepSelection::P0Route {
                        feasible_distance: membership.minimum_distance,
                        qualifying_p0_count: membership.qualifying_p0_count,
                    },
                })
                .collect(),
        }
    } else {
        let mode = if executable.is_empty() {
            RankingMode::None
        } else {
            RankingMode::Normal
        };
        Frontier {
            mode,
            steps: executable
                .into_values()
                .map(|issue| EvaluatedStep {
                    issue,
                    selection: StepSelection::Normal,
                })
                .collect(),
        }
    }
}

struct RouteMembership {
    minimum_distance: NonZeroUsize,
    qualifying_p0_count: NonZeroUsize,
}

impl RouteMembership {
    fn new(distance: NonZeroUsize) -> Self {
        Self {
            minimum_distance: distance,
            qualifying_p0_count: NonZeroUsize::MIN,
        }
    }

    fn include(&mut self, distance: NonZeroUsize) {
        self.minimum_distance = self.minimum_distance.min(distance);
        self.qualifying_p0_count = self
            .qualifying_p0_count
            .checked_add(1)
            .expect("P0 route membership count fits usize");
    }
}

fn snapshot<'a>(
    partial: &PartialRollout<'a>,
    graph: &crate::operational::OperationalGraph<'a>,
    pagerank: Option<&PageRank>,
    horizon: u8,
) -> EvaluatedCandidate<'a> {
    let issue = partial
        .steps
        .first()
        .expect("a recorded rollout has a first step")
        .issue;
    let unlocks: Vec<_> = partial
        .unlocks
        .iter()
        .filter_map(|number| graph.issue(*number))
        .collect();
    let mut priority_profile = PriorityProfile::default();
    for unlocked in &unlocks {
        let unlocked_priority = priority(unlocked);
        if unlocked_priority != PriorityComparison::P0 {
            priority_profile.record(unlocked_priority);
        }
    }
    let mut unlock_curve = partial.unlock_curve.clone();
    unlock_curve.resize(horizon as usize, unlock_curve.last().copied().unwrap_or(0));
    let mut p0_curve = partial.p0_curve.clone();
    p0_curve.resize(horizon as usize, p0_curve.last().copied().unwrap_or(0));
    let mut step_priorities: Vec<_> = partial
        .steps
        .iter()
        .map(|step| StepPriority::from(priority(step.issue)))
        .collect();
    step_priorities.resize(horizon as usize, StepPriority::NoStep);
    let candidate = CandidateData {
        issue,
        steps: partial.steps.clone(),
        unlocks,
        priority_profile,
        unlock_curve,
        p0_curve,
        step_priorities,
        pagerank_bucket: pagerank.and_then(|pagerank| pagerank.bucket(issue.number)),
    };
    match partial.steps.first().map(|step| step.selection) {
        Some(StepSelection::P0Route {
            feasible_distance,
            qualifying_p0_count,
        }) => EvaluatedCandidate::CriticalRoute {
            route: CriticalRouteOutcome {
                feasible_distance,
                realized_distance: candidate
                    .p0_curve
                    .iter()
                    .position(|count| *count > 0)
                    .and_then(|index| NonZeroUsize::new(index + 1)),
                qualifying_p0_count,
            },
            candidate,
        },
        Some(StepSelection::P0Ready) => EvaluatedCandidate::P0Ready(candidate),
        Some(StepSelection::Normal) => EvaluatedCandidate::Normal(candidate),
        None => unreachable!("a recorded rollout has a first step"),
    }
}

fn compare_same_first(left: &EvaluatedCandidate<'_>, right: &EvaluatedCandidate<'_>) -> Ordering {
    let outcome = super::decision::compare(left, right).ordering;
    if outcome != Ordering::Equal {
        return outcome;
    }
    let left_sequence: Vec<_> = left
        .data()
        .steps
        .iter()
        .map(|step| step.issue.number)
        .collect();
    let right_sequence: Vec<_> = right
        .data()
        .steps
        .iter()
        .map(|step| step.issue.number)
        .collect();
    right_sequence.cmp(&left_sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{BlockerIdentity, BlockerScope, Dependency, IssueIdentity, Label, LocalReplica},
        operational::OperationalGraph,
    };

    #[test]
    fn dense_p0_routes_complete_the_bounded_end_to_end_evaluation() {
        const ROUTE_COUNT: u64 = 5_000;
        let mut issues = Vec::with_capacity((ROUTE_COUNT * 2) as usize);
        let mut dependencies = Vec::with_capacity(ROUTE_COUNT as usize);
        for root in 1..=ROUTE_COUNT {
            let target = ROUTE_COUNT + root;
            issues.push(issue(root, false));
            issues.push(issue(target, true));
            dependencies.push(dependency(target, root));
        }
        let replica = LocalReplica {
            schema_version: "grit.local-replica/v1".to_owned(),
            repository: "acme/dense-p0".to_owned(),
            synced_at: "2026-08-07T00:00:00Z".to_owned(),
            input_hash: "fixture".to_owned(),
            repository_labels: Some(Vec::new()),
            issues,
            dependencies,
        };
        let graph = OperationalGraph::prepare(&replica);
        let result = evaluate(&graph, ExecutionScope::Available, None, 3, 8_192);

        assert_eq!(result.mode, RankingMode::P0Route);
        assert_eq!(result.candidate_count, ROUTE_COUNT as usize);
        assert_eq!(result.candidates.len(), ROUTE_COUNT as usize);
        assert_eq!(result.truncated_by.len(), 1);
        assert_eq!(result.truncated_by[0].as_str(), "state_budget");
    }

    fn issue(number: u64, p0: bool) -> crate::model::Issue {
        crate::model::Issue {
            id: number,
            node_id: format!("I_{number}"),
            number,
            url: format!("https://github.com/acme/dense-p0/issues/{number}"),
            title: format!("Issue {number}"),
            body: String::new(),
            state: "open".to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: p0
                .then(|| Label {
                    id: None,
                    node_id: None,
                    name: "priority:p0".to_owned(),
                    color: None,
                    description: None,
                })
                .into_iter()
                .collect(),
            comments: Vec::new(),
            created_at: "2026-08-01T00:00:00Z".to_owned(),
            updated_at: "2026-08-01T00:00:00Z".to_owned(),
            closed_at: None,
        }
    }

    fn dependency(blocked: u64, blocker: u64) -> Dependency {
        Dependency {
            blocked: IssueIdentity {
                repository: "acme/dense-p0".to_owned(),
                number: blocked,
                id: blocked,
                node_id: format!("I_{blocked}"),
            },
            blocker: BlockerIdentity {
                repository: "acme/dense-p0".to_owned(),
                number: blocker,
                state: "open".to_owned(),
                scope: BlockerScope::Internal,
                id: Some(blocker),
                node_id: Some(format!("I_{blocker}")),
            },
        }
    }
}
