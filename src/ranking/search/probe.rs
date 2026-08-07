use super::*;

impl<'graph, 'issues, 'scope, 'pagerank> Search<'graph, 'issues, 'scope, 'pagerank> {
    pub(super) fn probe(
        &mut self,
        parent: &SearchState<'graph, 'issues, 'scope>,
        anchor: EvaluatedStep<'issues>,
    ) -> Option<PartialRollout<'issues>> {
        if self.probe_work >= PROBE_WORK_BUDGET {
            self.restrictions.insert(SearchRestriction::ProbeBudget);
            return None;
        }
        let mut local_work = 0usize;
        let initial = self.probe_successor(parent, anchor, &mut local_work)?;
        Some(self.search_probe(initial, local_work))
    }

    pub(super) fn probe_from_state(
        &mut self,
        state: SearchState<'graph, 'issues, 'scope>,
    ) -> Option<PartialRollout<'issues>> {
        if self.probe_work >= PROBE_WORK_BUDGET {
            self.restrictions.insert(SearchRestriction::ProbeBudget);
            return None;
        }
        Some(self.search_probe(state, 0))
    }

    fn search_probe(
        &mut self,
        initial: SearchState<'graph, 'issues, 'scope>,
        mut local_work: usize,
    ) -> PartialRollout<'issues> {
        let mut best = initial.partial.clone();
        let mut beam = vec![initial];

        while beam
            .first()
            .is_some_and(|state| state.partial.steps.len() < self.horizon as usize)
            && local_work < PROBE_LOCAL_LIMIT
        {
            let mut successors = Vec::new();
            for state in &beam {
                let remaining = self.horizon as usize - state.partial.steps.len();
                let mut next = frontier(&state.rollout, remaining, &self.p0_targets);
                if next.mode == RankingMode::Normal {
                    next.steps
                        .retain(|step| state.causal.contains(step.issue.number));
                }
                if next.steps.is_empty() {
                    continue;
                }
                let scored = self.score_steps(state, next, false, STATE_POTENTIAL_VISITS);
                let causal = ranked_indices(&scored, |left, right| {
                    state
                        .causal
                        .category(left.step.issue.number)
                        .cmp(&state.causal.category(right.step.issue.number))
                        .then_with(|| self.compare_immediate(left, right))
                });
                let realized =
                    ranked_indices(&scored, |left, right| self.compare_immediate(left, right));
                let upper =
                    ranked_indices(&scored, |left, right| self.compare_upper_steps(left, right));
                let selected = quota_union(&[(&causal, 2), (&realized, 3), (&upper, 3)], 8);
                let selected = take_selected(scored, &selected);
                for step in selected {
                    let Some(successor) = self.probe_successor(state, step.step, &mut local_work)
                    else {
                        break;
                    };
                    if self.compare_partials(&successor.partial, &best).is_gt() {
                        best = successor.partial.clone();
                    }
                    successors.push(successor);
                }
                if local_work >= PROBE_LOCAL_LIMIT
                    || self.restrictions.contains(SearchRestriction::ProbeBudget)
                {
                    break;
                }
            }
            if successors.is_empty() {
                break;
            }
            let causal = ranked_indices(&successors, |left, right| {
                left.causal
                    .realized_continuations
                    .cmp(&right.causal.realized_continuations)
                    .then_with(|| self.compare_states(left, right))
            });
            let realized =
                ranked_indices(&successors, |left, right| self.compare_states(left, right));
            let upper = ranked_indices(&successors, |left, right| {
                self.compare_state_upper(left, right)
            });
            let selected = quota_union(&[(&causal, 2), (&realized, 3), (&upper, 3)], 8);
            beam = take_selected(successors, &selected);
        }
        best
    }

    fn probe_successor(
        &mut self,
        parent: &SearchState<'graph, 'issues, 'scope>,
        step: EvaluatedStep<'issues>,
        local_work: &mut usize,
    ) -> Option<SearchState<'graph, 'issues, 'scope>> {
        if *local_work >= PROBE_LOCAL_LIMIT || self.probe_work >= PROBE_WORK_BUDGET {
            if self.probe_work >= PROBE_WORK_BUDGET {
                self.restrictions.insert(SearchRestriction::ProbeBudget);
            }
            return None;
        }
        *local_work += 1;
        self.probe_work += 1;
        self.materialize(parent, step, None, false)
    }
}
