use super::*;

impl<'graph, 'issues, 'scope, 'pagerank, 'working>
    Search<'graph, 'issues, 'scope, 'pagerank, 'working>
{
    pub(super) fn score_steps(
        &mut self,
        parent: &SearchState<'graph, 'issues, 'scope>,
        mut frontier: Frontier<'issues>,
        include_structural: bool,
        potential_visit_limit: usize,
    ) -> Vec<ScoredStep<'issues>> {
        let remaining_after = self.horizon as usize - parent.partial.steps.len() - 1;
        let mut rollout = parent.rollout.clone();
        let mut partial = parent.partial.clone();
        let mut scored = Vec::with_capacity(frontier.steps.len());
        frontier
            .steps
            .sort_by_key(|step| step.issue.stable_node_key());
        for step in frontier.steps {
            let completion = rollout
                .complete(step.issue.number)
                .expect("scored frontiers contain only Executable Issues");
            let checkpoint = partial.apply(
                step.clone(),
                completion.newly_ready(),
                rollout.graph(),
                self.working,
            );
            self.track_partial(&partial);
            let upper =
                self.potential_key(&rollout, &partial, remaining_after, potential_visit_limit);
            let structural = if include_structural {
                self.structural_key(step.issue.number)
            } else {
                StructuralKey::default()
            };
            scored.push(ScoredStep {
                step,
                immediate: partial.clone(),
                structural,
                upper,
                joint_plan: None,
            });
            partial.undo(checkpoint);
            rollout.undo(completion);
        }
        scored
    }

    pub(super) fn compare_immediate(
        &self,
        left: &ScoredStep<'issues>,
        right: &ScoredStep<'issues>,
    ) -> Ordering {
        self.compare_partials(&left.immediate, &right.immediate)
    }

    pub(super) fn compare_pagerank(
        &self,
        left: &ScoredStep<'issues>,
        right: &ScoredStep<'issues>,
    ) -> Ordering {
        self.pagerank
            .and_then(|pagerank| pagerank.bucket(left.step.issue.number))
            .cmp(
                &self
                    .pagerank
                    .and_then(|pagerank| pagerank.bucket(right.step.issue.number)),
            )
            .then_with(|| self.compare_immediate(left, right))
            .then_with(|| {
                right
                    .step
                    .issue
                    .stable_node_key()
                    .cmp(&left.step.issue.stable_node_key())
            })
    }

    pub(super) fn compare_joint(
        &self,
        left: &ScoredStep<'issues>,
        right: &ScoredStep<'issues>,
    ) -> Ordering {
        match (&left.joint_plan, &right.joint_plan) {
            (Some(left_plan), Some(right_plan)) => self
                .compare_partials(left_plan, right_plan)
                .then_with(|| self.compare_immediate(left, right))
                .then_with(|| self.compare_pagerank(left, right)),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => self.compare_immediate(left, right),
        }
    }

    pub(super) fn compare_upper_steps(
        &self,
        left: &ScoredStep<'issues>,
        right: &ScoredStep<'issues>,
    ) -> Ordering {
        self.compare_potential(&left.upper, &left.immediate, &right.upper, &right.immediate)
    }

    pub(super) fn compare_p0_discovery(
        &self,
        left: &ScoredStep<'issues>,
        right: &ScoredStep<'issues>,
    ) -> Ordering {
        let left_route = route_discovery_key(left.step.selection);
        let right_route = route_discovery_key(right.step.selection);
        right_route
            .0
            .cmp(&left_route.0)
            .then_with(|| left_route.1.cmp(&right_route.1))
            .then_with(|| left.upper.p0_curve.cmp(&right.upper.p0_curve))
            .then_with(|| self.compare_immediate(left, right))
            .then_with(|| self.compare_pagerank(left, right))
            .then_with(|| {
                right
                    .step
                    .issue
                    .stable_node_key()
                    .cmp(&left.step.issue.stable_node_key())
            })
    }

    pub(super) fn compare_states(
        &self,
        left: &SearchState<'graph, 'issues, 'scope>,
        right: &SearchState<'graph, 'issues, 'scope>,
    ) -> Ordering {
        self.compare_partials(&left.partial, &right.partial)
    }

    pub(super) fn compare_state_joint(
        &self,
        left: &SearchState<'graph, 'issues, 'scope>,
        right: &SearchState<'graph, 'issues, 'scope>,
    ) -> Ordering {
        match (&left.joint_plan, &right.joint_plan) {
            (Some(left_plan), Some(right_plan)) => self
                .compare_partials(left_plan, right_plan)
                .then_with(|| self.compare_states(left, right)),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => self.compare_states(left, right),
        }
    }

    pub(super) fn compare_state_upper(
        &self,
        left: &SearchState<'graph, 'issues, 'scope>,
        right: &SearchState<'graph, 'issues, 'scope>,
    ) -> Ordering {
        self.compare_potential(&left.upper, &left.partial, &right.upper, &right.partial)
    }

    pub(super) fn compare_partials(
        &self,
        left: &PartialRollout<'issues>,
        right: &PartialRollout<'issues>,
    ) -> Ordering {
        let left = snapshot(
            left,
            self.root.graph(),
            self.pagerank,
            self.horizon,
            self.working,
        );
        let right = snapshot(
            right,
            self.root.graph(),
            self.pagerank,
            self.horizon,
            self.working,
        );
        compare_same_first(&left, &right)
    }

    fn compare_potential(
        &self,
        left: &PotentialKey,
        left_partial: &PartialRollout<'issues>,
        right: &PotentialKey,
        right_partial: &PartialRollout<'issues>,
    ) -> Ordering {
        let p0_mode = left_partial
            .steps
            .first()
            .is_some_and(|step| step.selection.mode().is_p0());
        (if p0_mode {
            left.p0_curve.cmp(&right.p0_curve)
        } else {
            Ordering::Equal
        })
        .then_with(|| left.unlock_count.cmp(&right.unlock_count))
        .then_with(|| left.priority_profile.cmp(&right.priority_profile))
        .then_with(|| left.unlock_curve.cmp(&right.unlock_curve))
        .then_with(|| self.compare_partials(left_partial, right_partial))
    }

    pub(super) fn potential_key(
        &mut self,
        rollout: &RolloutState<'graph, 'issues, 'scope>,
        partial: &PartialRollout<'issues>,
        remaining_steps: usize,
        visit_limit: usize,
    ) -> PotentialKey {
        let graph = rollout.graph();
        let mut potential_counts = vec![0usize; remaining_steps + 1];
        let mut potential_p0_counts = vec![0usize; remaining_steps + 1];
        let mut potential_priority_profiles = vec![[0usize; 4]; remaining_steps + 1];
        let mut visits = 0usize;
        let mut exhausted = false;
        for number in self
            .potential_targets
            .iter()
            .filter(|_| remaining_steps > 0)
        {
            if rollout.is_completed(*number)
                || rollout.is_ready(*number)
                || partial.unlocks.contains(number)
            {
                continue;
            }
            if visits >= visit_limit {
                exhausted = true;
                break;
            }
            visits += 1;
            let Some(closure) = self
                .feasible_closures
                .get(number)
                .and_then(|closure| closure.as_ref())
            else {
                continue;
            };
            let distance = closure
                .iter()
                .filter(|blocker| !rollout.is_completed(**blocker))
                .count();
            if distance == 0 || distance > remaining_steps {
                continue;
            }
            visits = visits.saturating_add(distance);
            if visits > visit_limit {
                exhausted = true;
                break;
            }
            potential_counts[distance] += 1;
            if let Some(issue) = graph.issue(*number) {
                let issue_priority = self.priority(issue);
                if issue_priority == PriorityComparison::P0 {
                    potential_p0_counts[distance] += 1;
                } else {
                    let mut profile = PriorityProfile::default();
                    profile.record(issue_priority);
                    let profile = profile.as_array();
                    for (total, increment) in potential_priority_profiles[distance]
                        .iter_mut()
                        .zip(profile)
                    {
                        *total += increment;
                    }
                }
            }
        }
        if exhausted {
            self.restrictions.insert(SearchRestriction::PotentialBudget);
        }

        let depth = partial.steps.len();
        let mut unlock_curve = partial.unlock_curve.clone();
        let mut p0_curve = partial.p0_curve.clone();
        let partial_p0_count = partial
            .unlocks
            .iter()
            .filter_map(|number| graph.issue(*number))
            .filter(|issue| self.priority(issue) == PriorityComparison::P0)
            .count();
        let mut cumulative_count = 0usize;
        let mut cumulative_p0_count = 0usize;
        for future_step in 1..=remaining_steps {
            cumulative_count += potential_counts[future_step];
            cumulative_p0_count += potential_p0_counts[future_step];
            unlock_curve.push(partial.unlocks.len() + cumulative_count);
            p0_curve.push(partial_p0_count + cumulative_p0_count);
        }
        unlock_curve.resize(self.horizon as usize, partial.unlocks.len());
        p0_curve.resize(self.horizon as usize, partial_p0_count);
        debug_assert_eq!(unlock_curve.len(), self.horizon as usize);
        debug_assert_eq!(p0_curve.len(), self.horizon as usize);
        debug_assert_eq!(depth + remaining_steps, self.horizon as usize);

        let mut profile = PriorityProfile::default();
        for number in &partial.unlocks {
            if let Some(issue) = graph.issue(*number) {
                let issue_priority = self.priority(issue);
                if issue_priority != PriorityComparison::P0 {
                    profile.record(issue_priority);
                }
            }
        }
        let mut profile = profile.as_array();
        for distance_profile in potential_priority_profiles
            .iter()
            .take(remaining_steps + 1)
            .skip(1)
        {
            for (total, increment) in profile.iter_mut().zip(distance_profile) {
                *total += increment;
            }
        }
        PotentialKey {
            p0_curve,
            unlock_count: partial.unlocks.len() + cumulative_count,
            priority_profile: profile,
            unlock_curve,
        }
    }

    fn structural_key(&mut self, seed: u64) -> StructuralKey {
        let graph = self.root.graph();
        let mut queue = VecDeque::new();
        for dependent in graph.dependents_for(seed) {
            queue.push_back((*dependent, 1usize));
        }
        let mut reached = BTreeSet::new();
        let mut visits = 0usize;
        while let Some((number, depth)) = queue.pop_front() {
            if reached.contains(&number) {
                continue;
            }
            if visits >= INITIAL_POTENTIAL_VISITS {
                self.restrictions.insert(SearchRestriction::PotentialBudget);
                break;
            }
            reached.insert(number);
            visits += 1;
            if depth < self.horizon as usize {
                for dependent in graph.dependents_for(number) {
                    queue.push_back((*dependent, depth + 1));
                }
            }
        }
        let mut profile = [0usize; 5];
        for number in &reached {
            if let Some(issue) = graph.issue(*number) {
                profile[priority_profile_index(self.priority(issue))] += 1;
            }
        }
        StructuralKey {
            count: reached.len(),
            priority_profile: profile,
        }
    }
}

fn route_discovery_key(selection: StepSelection) -> (usize, usize) {
    match selection {
        StepSelection::P0Ready => (0, 1),
        StepSelection::P0Route {
            feasible_distance,
            qualifying_p0_count,
        } => (feasible_distance.get(), qualifying_p0_count.get()),
        StepSelection::Normal => (usize::MAX, 0),
    }
}

fn priority_profile_index(priority: PriorityComparison) -> usize {
    match priority {
        PriorityComparison::P0 => 0,
        PriorityComparison::P1 => 1,
        PriorityComparison::Neutral => 2,
        PriorityComparison::P3 => 3,
        PriorityComparison::P4 => 4,
    }
}
