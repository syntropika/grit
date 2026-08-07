mod frontier;
mod policy;
mod probe;
mod scoring;
mod snapshot;
mod state;

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
    num::NonZeroUsize,
};

use frontier::*;
pub(super) use policy::SearchRestriction;
use policy::*;
use snapshot::*;
use state::*;

use super::{
    CandidateData, CriticalRouteOutcome, EvaluatedCandidate, EvaluatedStep, StepSelection,
    decision::{PriorityProfile, RankingMode, StepPriority},
    pagerank::PageRank,
    priority,
};
use crate::{
    model::BlockerScope,
    operational::{ExecutionScope, OneStepAnalysis, RolloutState},
    priority::PriorityComparison,
};

pub(super) struct SearchResult<'a> {
    pub(super) mode: RankingMode,
    pub(super) candidate_count: usize,
    pub(super) candidates: Vec<EvaluatedCandidate<'a>>,
    pub(super) truncated_by: Vec<SearchRestriction>,
}

struct Search<'graph, 'issues, 'scope, 'pagerank> {
    root: RolloutState<'graph, 'issues, 'scope>,
    pagerank: Option<&'pagerank PageRank>,
    horizon: u8,
    state_budget: usize,
    expanded_states: usize,
    probe_work: usize,
    p0_targets: BTreeSet<u64>,
    potential_targets: Vec<u64>,
    feasible_closures: BTreeMap<u64, Option<Vec<u64>>>,
    restrictions: Restrictions,
    best_by_first: BTreeMap<u64, EvaluatedCandidate<'issues>>,
}

pub(super) fn evaluate<'graph, 'issues, 'scope, 'pagerank>(
    graph: &'graph crate::operational::OperationalGraph<'issues>,
    scope: ExecutionScope<'scope>,
    pagerank: Option<&'pagerank PageRank>,
    horizon: u8,
    state_budget: usize,
) -> SearchResult<'issues> {
    let root = graph.rollout_state(scope);
    let p0_targets = graph
        .open_numbers()
        .iter()
        .copied()
        .filter(|number| {
            graph
                .issue(*number)
                .is_some_and(|issue| priority(issue) == PriorityComparison::P0)
        })
        .collect::<BTreeSet<_>>();
    let potential_targets = if horizon == 1 {
        Vec::new()
    } else {
        graph
            .open_numbers()
            .iter()
            .copied()
            .filter(|number| !root.is_ready(*number))
            .collect()
    };
    let feasible_closures = potential_targets
        .iter()
        .map(|number| {
            (
                *number,
                root.feasible_prerequisite_closure(*number, horizon as usize)
                    .map(|closure| closure.into_iter().collect()),
            )
        })
        .collect();
    let search = Search {
        root,
        pagerank,
        horizon,
        state_budget,
        expanded_states: 0,
        probe_work: 0,
        p0_targets,
        potential_targets,
        feasible_closures,
        restrictions: Restrictions::default(),
        best_by_first: BTreeMap::new(),
    };
    search.run()
}

impl<'graph, 'issues, 'scope, 'pagerank> Search<'graph, 'issues, 'scope, 'pagerank> {
    fn run(mut self) -> SearchResult<'issues> {
        let one_step_analysis = (self.horizon == 1).then(|| self.root.one_step_analysis());
        let first_frontier = if let Some(analysis) = &one_step_analysis {
            one_step_frontier(analysis, &self.p0_targets)
        } else {
            frontier(&self.root, self.horizon as usize, &self.p0_targets)
        };
        let mode = first_frontier.mode;
        if self.horizon == 1 {
            let candidate_count = first_frontier.steps.len();
            let mut partial = SearchState::root(self.root.clone(), self.horizon).partial;
            for step in first_frontier.steps {
                let newly_ready = one_step_analysis
                    .as_ref()
                    .map(|analysis| analysis.unlocks_for(step.issue.number))
                    .unwrap_or(&[]);
                let checkpoint = partial.apply(step, newly_ready, self.root.graph());
                self.record(&partial);
                partial.undo(checkpoint);
            }
            return self.finish(mode, candidate_count);
        }

        let root = SearchState::root(self.root.clone(), self.horizon);
        let first_steps = self.select_first_steps(&root, first_frontier);
        let candidate_count = first_steps.len();
        let mut beam = Vec::with_capacity(candidate_count);
        for scored in first_steps {
            if let Some(state) = self.materialize(&root, scored.step, scored.joint_plan, true) {
                self.record(&state.partial);
                beam.push(state);
            } else {
                break;
            }
        }
        beam = self.retain_beam(beam);

        while beam
            .first()
            .is_some_and(|state| state.partial.steps.len() < self.horizon as usize)
            && !self.restrictions.contains(SearchRestriction::StateBudget)
        {
            let mut successors = Vec::new();
            for state in &beam {
                let remaining = self.horizon as usize - state.partial.steps.len();
                let next_frontier = frontier(&state.rollout, remaining, &self.p0_targets);
                let branches = self.select_branches(state, next_frontier);
                for scored in branches {
                    if let Some(successor) =
                        self.materialize(state, scored.step, scored.joint_plan, true)
                    {
                        self.record(&successor.partial);
                        successors.push(successor);
                    } else {
                        break;
                    }
                }
                if self.restrictions.contains(SearchRestriction::StateBudget) {
                    break;
                }
            }
            if successors.is_empty() {
                break;
            }
            beam = self.retain_beam(successors);
        }

        self.finish(mode, candidate_count)
    }

    fn record(&mut self, partial: &PartialRollout<'issues>) {
        let candidate = snapshot(partial, self.root.graph(), self.pagerank, self.horizon);
        let first_number = candidate.data().issue.number;
        let replace = self
            .best_by_first
            .get(&first_number)
            .is_none_or(|current| compare_same_first(&candidate, current).is_gt());
        if replace {
            self.best_by_first.insert(first_number, candidate);
        }
    }

    fn finish(self, mode: RankingMode, candidate_count: usize) -> SearchResult<'issues> {
        SearchResult {
            mode,
            candidate_count,
            candidates: self.best_by_first.into_values().collect(),
            truncated_by: self.restrictions.into_vec(),
        }
    }

    fn materialize(
        &mut self,
        parent: &SearchState<'graph, 'issues, 'scope>,
        step: EvaluatedStep<'issues>,
        joint_plan: Option<PartialRollout<'issues>>,
        counts_against_budget: bool,
    ) -> Option<SearchState<'graph, 'issues, 'scope>> {
        if counts_against_budget {
            if self.expanded_states >= self.state_budget {
                self.restrictions.insert(SearchRestriction::StateBudget);
                return None;
            }
            self.expanded_states += 1;
        }
        let mut rollout = parent.rollout.clone();
        let completion = rollout
            .complete(step.issue.number)
            .expect("search frontiers contain only Executable Issues");
        let mut partial = parent.partial.clone();
        partial.apply(step.clone(), completion.newly_ready(), rollout.graph());
        let mut causal = parent.causal.clone();
        causal.advance(step.issue.number, completion.newly_ready(), rollout.graph());
        let remaining = self.horizon as usize - partial.steps.len();
        let upper = self.potential_key(&rollout, &partial, remaining, STATE_POTENTIAL_VISITS);
        Some(SearchState {
            rollout,
            partial,
            causal,
            joint_plan,
            upper,
        })
    }

    fn select_first_steps(
        &mut self,
        root: &SearchState<'graph, 'issues, 'scope>,
        frontier: Frontier<'issues>,
    ) -> Vec<ScoredStep<'issues>> {
        let mode = frontier.mode;
        let mut scored = self.score_steps(root, frontier, !mode.is_p0(), INITIAL_POTENTIAL_VISITS);
        if mode.is_p0() {
            let ranked = ranked_indices(&scored, |left, right| {
                self.compare_p0_discovery(left, right)
            });
            if ranked.len() > FIRST_STEP_LIMIT {
                self.restrictions.insert(SearchRestriction::P0Frontier);
            }
            return take_selected(scored, &ranked[..ranked.len().min(FIRST_STEP_LIMIT)]);
        }

        let immediate = ranked_indices(&scored, |left, right| self.compare_immediate(left, right));
        let structural = ranked_indices(&scored, |left, right| {
            left.structural
                .count
                .cmp(&right.structural.count)
                .then_with(|| {
                    left.structural
                        .priority_profile
                        .cmp(&right.structural.priority_profile)
                })
                .then_with(|| self.compare_immediate(left, right))
        });
        let by_priority = ranked_indices(&scored, |left, right| {
            StepPriority::from(priority(left.step.issue))
                .cmp(&StepPriority::from(priority(right.step.issue)))
                .then_with(|| self.compare_immediate(left, right))
        });
        let by_pagerank = if self.pagerank.is_some() {
            ranked_indices(&scored, |left, right| self.compare_pagerank(left, right))
        } else {
            Vec::new()
        };
        let probe_pool = quota_union(
            &[
                (&immediate, 128),
                (&structural, 64),
                (&by_priority, 32),
                (&by_pagerank, 32),
            ],
            256,
        );
        if probe_pool.len() < scored.len() {
            self.restrictions.insert(SearchRestriction::ProbePool);
        }
        for index in probe_pool {
            scored[index].joint_plan = self.probe(root, scored[index].step.clone());
        }
        let joint = ranked_indices_filtered(
            &scored,
            |step| step.joint_plan.is_some(),
            |left, right| self.compare_joint(left, right),
        );
        let shortlist = quota_union(
            &[
                (&immediate, 32),
                (&structural, 16),
                (&joint, 16),
                (&by_priority, 16),
                (&by_pagerank, 16),
            ],
            FIRST_STEP_LIMIT,
        );
        if shortlist.len() < scored.len() {
            self.restrictions
                .insert(SearchRestriction::FirstStepShortlist);
        }
        take_selected(scored, &shortlist)
    }

    fn select_branches(
        &mut self,
        parent: &SearchState<'graph, 'issues, 'scope>,
        frontier: Frontier<'issues>,
    ) -> Vec<ScoredStep<'issues>> {
        let mode = frontier.mode;
        let mut scored = self.score_steps(parent, frontier, false, STATE_POTENTIAL_VISITS);
        let immediate = ranked_indices(&scored, |left, right| self.compare_immediate(left, right));
        let upper = ranked_indices(&scored, |left, right| self.compare_upper_steps(left, right));
        let causal_immediate = ranked_indices_filtered(
            &scored,
            |step| mode.is_p0() || parent.causal.contains(step.step.issue.number),
            |left, right| self.compare_immediate(left, right),
        );
        let causal_upper = ranked_indices_filtered(
            &scored,
            |step| mode.is_p0() || parent.causal.contains(step.step.issue.number),
            |left, right| self.compare_upper_steps(left, right),
        );
        let probe_pool = quota_union(
            &[
                (&immediate, 8),
                (&upper, 8),
                (&causal_immediate, 8),
                (&causal_upper, 8),
            ],
            32,
        );
        if probe_pool.len() < scored.len() {
            self.restrictions.insert(SearchRestriction::ProbePool);
        }
        for index in probe_pool {
            scored[index].joint_plan = self.probe(parent, scored[index].step.clone());
        }
        let joint = ranked_indices_filtered(
            &scored,
            |step| step.joint_plan.is_some(),
            |left, right| self.compare_joint(left, right),
        );
        let selected = quota_union(&[(&immediate, 16), (&joint, 8), (&upper, 8)], BRANCH_LIMIT);
        if selected.len() < scored.len() {
            self.restrictions.insert(SearchRestriction::BranchWidth);
        }
        take_selected(scored, &selected)
    }

    fn retain_beam(
        &mut self,
        mut states: Vec<SearchState<'graph, 'issues, 'scope>>,
    ) -> Vec<SearchState<'graph, 'issues, 'scope>> {
        if states.len() <= BEAM_LIMIT {
            return states;
        }
        self.restrictions.insert(SearchRestriction::BeamWidth);
        let realized_ranked =
            ranked_indices(&states, |left, right| self.compare_states(left, right));
        let upper_ranked =
            ranked_indices(&states, |left, right| self.compare_state_upper(left, right));
        let first_issue = |state: &SearchState<'_, '_, '_>| {
            state
                .partial
                .steps
                .first()
                .expect("beam states have a first step")
                .issue
                .number
        };
        let realized_probe = diversify(&states, realized_ranked.clone(), first_issue);
        let upper_probe = diversify(&states, upper_ranked.clone(), first_issue);
        let probe_pool = quota_union(&[(&realized_probe, 32), (&upper_probe, 32)], 64);
        if probe_pool.len() < states.len() {
            self.restrictions.insert(SearchRestriction::ProbePool);
        }
        for index in probe_pool {
            if states[index].joint_plan.is_none()
                && states[index].partial.steps.len() < self.horizon as usize
            {
                let state = states[index].clone();
                states[index].joint_plan = self.probe_from_state(state);
            }
        }
        let realized = diversify(&states, realized_ranked, first_issue);
        let joint = diversify(
            &states,
            ranked_indices_filtered(
                &states,
                |state| state.joint_plan.is_some(),
                |left, right| self.compare_state_joint(left, right),
            ),
            first_issue,
        );
        let upper = diversify(&states, upper_ranked, first_issue);
        let selected = quota_union(&[(&realized, 32), (&joint, 16), (&upper, 16)], BEAM_LIMIT);
        take_selected(states, &selected)
    }
}

#[cfg(test)]
mod tests;
