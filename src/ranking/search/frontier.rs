use super::*;

pub(in crate::ranking) struct Frontier<'a> {
    pub(in crate::ranking) mode: RankingMode,
    pub(in crate::ranking) steps: Vec<EvaluatedStep<'a>>,
}

pub(in crate::ranking) fn frontier<'issues>(
    state: &RolloutState<'_, 'issues, '_>,
    remaining_steps: usize,
    p0_targets: &BTreeSet<u64>,
) -> Frontier<'issues> {
    let executable = state.executable();
    if let Some(frontier) = p0_ready_frontier(&executable, p0_targets) {
        return frontier;
    }
    if p0_targets.is_empty() {
        return normal_frontier(executable);
    }
    let executable: BTreeMap<_, _> = executable
        .into_iter()
        .map(|issue| (issue.number, issue))
        .collect();

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
    finish_frontier(executable, route_by_step)
}

pub(super) fn one_step_frontier<'issues>(
    analysis: &OneStepAnalysis<'issues>,
    p0_targets: &BTreeSet<u64>,
) -> Frontier<'issues> {
    let executable = analysis
        .executable()
        .map(|issue| (issue.number, issue))
        .collect::<BTreeMap<_, _>>();
    let executable_issues = executable.values().copied().collect::<Vec<_>>();
    if let Some(frontier) = p0_ready_frontier(&executable_issues, p0_targets) {
        return frontier;
    }
    let route_by_step = analysis
        .unlocks()
        .filter_map(|(step_number, newly_ready)| {
            let qualifying_p0_count = NonZeroUsize::new(
                newly_ready
                    .iter()
                    .filter(|number| p0_targets.contains(number))
                    .count(),
            )?;
            Some((
                step_number,
                RouteMembership {
                    minimum_distance: NonZeroUsize::MIN,
                    qualifying_p0_count,
                },
            ))
        })
        .collect();
    finish_frontier(executable, route_by_step)
}

fn p0_ready_frontier<'issues>(
    executable: &[&'issues crate::model::Issue],
    p0_targets: &BTreeSet<u64>,
) -> Option<Frontier<'issues>> {
    let steps = executable
        .iter()
        .copied()
        .filter(|issue| p0_targets.contains(&issue.number))
        .map(|issue| EvaluatedStep {
            issue,
            selection: StepSelection::P0Ready,
        })
        .collect::<Vec<_>>();
    (!steps.is_empty()).then_some(Frontier {
        mode: RankingMode::P0Ready,
        steps,
    })
}

fn normal_frontier(executable: Vec<&crate::model::Issue>) -> Frontier<'_> {
    let mode = if executable.is_empty() {
        RankingMode::None
    } else {
        RankingMode::Normal
    };
    Frontier {
        mode,
        steps: executable
            .into_iter()
            .map(|issue| EvaluatedStep {
                issue,
                selection: StepSelection::Normal,
            })
            .collect(),
    }
}

fn finish_frontier<'issues>(
    executable: BTreeMap<u64, &'issues crate::model::Issue>,
    route_by_step: BTreeMap<u64, RouteMembership>,
) -> Frontier<'issues> {
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
