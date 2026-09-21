use super::*;

pub(super) fn snapshot<'a>(
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

pub(super) fn compare_same_first(
    left: &EvaluatedCandidate<'_>,
    right: &EvaluatedCandidate<'_>,
) -> Ordering {
    let outcome = super::super::decision::compare(left, right).ordering;
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
