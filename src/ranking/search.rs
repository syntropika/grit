use std::{cmp::Ordering, collections::BTreeMap, collections::BTreeSet};

use serde::Serialize;

use super::{
    EvaluatedCandidate, EvaluatedStep,
    decision::{PriorityProfile, RankingMode, StepPriority},
    pagerank::PageRank,
    priority,
};
use crate::{
    model::Issue,
    operational::{ExecutionScope, RolloutState},
    priority::{PriorityComparison, PriorityState},
    working_graph::WorkingGraph,
};

pub(super) struct SearchResult<'a> {
    pub(super) mode: RankingMode,
    pub(super) candidate_count: usize,
    pub(super) candidates: Vec<EvaluatedCandidate<'a>>,
    pub(super) truncated_by: Vec<SearchRestriction>,
    pub(super) provenance_numbers: BTreeSet<u64>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SearchRestriction {
    P0Frontier,
    StateBudget,
}

impl SearchRestriction {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::P0Frontier => "p0_frontier",
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
        working: &WorkingGraph<'_>,
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
            .filter(|issue| priority(working, issue) == PriorityComparison::P0)
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

struct Search<'graph, 'issues, 'scope, 'pagerank, 'working> {
    working: &'working WorkingGraph<'issues>,
    provenance_numbers: BTreeSet<u64>,
    state: RolloutState<'graph, 'issues, 'scope>,
    pagerank: Option<&'pagerank PageRank>,
    horizon: u8,
    state_budget: usize,
    expanded_states: usize,
    state_budget_exhausted: bool,
    initial_mode: RankingMode,
    best_by_first: BTreeMap<u64, EvaluatedCandidate<'issues>>,
}

pub(super) fn evaluate<'graph, 'issues, 'scope, 'pagerank>(
    working: &WorkingGraph<'issues>,
    graph: &'graph crate::operational::OperationalGraph<'issues>,
    scope: ExecutionScope<'scope>,
    pagerank: Option<&'pagerank PageRank>,
    horizon: u8,
    state_budget: usize,
    deferred_critical_routes: bool,
) -> SearchResult<'issues> {
    let state = graph.rollout_state(scope);
    let mut provenance_numbers = BTreeSet::new();
    let (initial_mode, first_frontier) = frontier(&state, working, &mut provenance_numbers);
    let candidate_count = first_frontier.len();
    let first_steps_count_against_budget = horizon > 1;
    let mut search = Search {
        working,
        provenance_numbers,
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
        initial_mode,
        best_by_first: BTreeMap::new(),
    };
    for first_issue in first_frontier {
        let mut partial = PartialRollout {
            steps: Vec::with_capacity(horizon as usize),
            unlocks: BTreeSet::new(),
            unlock_curve: Vec::with_capacity(horizon as usize),
            p0_curve: Vec::with_capacity(horizon as usize),
        };
        search.explore(first_issue, initial_mode, &mut partial, false);
    }
    SearchResult {
        mode: initial_mode,
        candidate_count,
        candidates: search.best_by_first.into_values().collect(),
        provenance_numbers: search.provenance_numbers,
        truncated_by: [
            deferred_critical_routes.then_some(SearchRestriction::P0Frontier),
            search
                .state_budget_exhausted
                .then_some(SearchRestriction::StateBudget),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

impl<'graph, 'issues, 'scope, 'pagerank, 'working>
    Search<'graph, 'issues, 'scope, 'pagerank, 'working>
{
    fn explore(
        &mut self,
        issue: &'issues Issue,
        mode: RankingMode,
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
            .complete(issue.number)
            .expect("search frontiers contain only Executable Issues");
        let checkpoint = partial.apply(
            EvaluatedStep { issue, mode },
            completion.newly_ready(),
            self.state.graph(),
            self.working,
        );
        self.record(partial);

        if partial.steps.len() < self.horizon as usize && !self.state_budget_exhausted {
            let (next_mode, next_frontier) =
                frontier(&self.state, self.working, &mut self.provenance_numbers);
            for next_issue in next_frontier {
                self.explore(next_issue, next_mode, partial, true);
                if self.state_budget_exhausted && self.expanded_states >= self.state_budget {
                    break;
                }
            }
        }

        partial.undo(checkpoint);
        self.state.undo(completion);
    }

    fn record(&mut self, partial: &PartialRollout<'issues>) {
        let candidate = snapshot(
            partial,
            self.state.graph(),
            self.pagerank,
            self.horizon,
            self.working,
        );
        let first_number = candidate.issue.number;
        let replace = self.best_by_first.get(&first_number).is_none_or(|current| {
            compare_same_first(&candidate, current, self.initial_mode).is_gt()
        });
        if replace {
            self.best_by_first.insert(first_number, candidate);
        }
    }
}

fn frontier<'issues>(
    state: &RolloutState<'_, 'issues, '_>,
    working: &WorkingGraph<'_>,
    provenance_numbers: &mut BTreeSet<u64>,
) -> (RankingMode, Vec<&'issues Issue>) {
    let executable = state.executable();
    for issue in &executable {
        for inspected in std::iter::once(*issue).chain(state.preview_unlocks(issue.number)) {
            let base = PriorityState::from_issue_labels(&inspected.labels).comparison();
            if (base == PriorityComparison::P0)
                != (priority(working, inspected) == PriorityComparison::P0)
            {
                provenance_numbers.insert(inspected.number);
            }
        }
    }
    let executable_p0: Vec<_> = executable
        .iter()
        .copied()
        .filter(|issue| priority(working, issue) == PriorityComparison::P0)
        .collect();
    if !executable_p0.is_empty() {
        track_candidates(state, &executable_p0, provenance_numbers);
        return (RankingMode::P0Ready, executable_p0);
    }
    let p0_routes: Vec<_> = executable
        .iter()
        .copied()
        .filter(|issue| {
            state
                .preview_unlocks(issue.number)
                .iter()
                .any(|unlocked| priority(working, unlocked) == PriorityComparison::P0)
        })
        .collect();
    if !p0_routes.is_empty() {
        track_candidates(state, &p0_routes, provenance_numbers);
        (RankingMode::P0Route, p0_routes)
    } else if executable.is_empty() {
        (RankingMode::None, Vec::new())
    } else {
        track_candidates(state, &executable, provenance_numbers);
        (RankingMode::Normal, executable)
    }
}

fn track_candidates(
    state: &RolloutState<'_, '_, '_>,
    candidates: &[&Issue],
    provenance_numbers: &mut BTreeSet<u64>,
) {
    for issue in candidates {
        provenance_numbers.insert(issue.number);
        provenance_numbers.extend(
            state
                .preview_unlocks(issue.number)
                .iter()
                .map(|issue| issue.number),
        );
    }
}

fn snapshot<'a>(
    partial: &PartialRollout<'a>,
    graph: &crate::operational::OperationalGraph<'a>,
    pagerank: Option<&PageRank>,
    horizon: u8,
    working: &WorkingGraph<'_>,
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
        let unlocked_priority = priority(working, unlocked);
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
        .map(|step| StepPriority::from(priority(working, step.issue)))
        .collect();
    step_priorities.resize(horizon as usize, StepPriority::NoStep);
    EvaluatedCandidate {
        issue,
        steps: partial.steps.clone(),
        unlocks,
        priority_profile,
        unlock_curve,
        p0_curve,
        step_priorities,
        pagerank_bucket: pagerank.and_then(|pagerank| pagerank.bucket(issue.number)),
    }
}

fn compare_same_first(
    left: &EvaluatedCandidate<'_>,
    right: &EvaluatedCandidate<'_>,
    mode: RankingMode,
) -> Ordering {
    let outcome = super::decision::compare(left, right, mode).ordering;
    if outcome != Ordering::Equal {
        return outcome;
    }
    let left_sequence: Vec<_> = left.steps.iter().map(|step| step.issue.number).collect();
    let right_sequence: Vec<_> = right.steps.iter().map(|step| step.issue.number).collect();
    right_sequence.cmp(&left_sequence)
}
