use std::{cmp::Ordering, collections::BTreeSet};

use serde::Serialize;

pub(super) const FIRST_STEP_LIMIT: usize = 96;
pub(super) const BEAM_LIMIT: usize = 64;
pub(super) const BRANCH_LIMIT: usize = 32;
pub(super) const PROBE_LOCAL_LIMIT: usize = 64;
pub(super) const PROBE_WORK_BUDGET: usize = 262_144;
pub(super) const INITIAL_POTENTIAL_VISITS: usize = 1_024;
pub(super) const STATE_POTENTIAL_VISITS: usize = 512;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchRestriction {
    FirstStepShortlist,
    PotentialBudget,
    ProbePool,
    ProbeBudget,
    P0Frontier,
    BranchWidth,
    BeamWidth,
    StateBudget,
}

impl SearchRestriction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FirstStepShortlist => "first_step_shortlist",
            Self::PotentialBudget => "potential_budget",
            Self::ProbePool => "probe_pool",
            Self::ProbeBudget => "probe_budget",
            Self::P0Frontier => "p0_frontier",
            Self::BranchWidth => "branch_width",
            Self::BeamWidth => "beam_width",
            Self::StateBudget => "state_budget",
        }
    }
}

#[derive(Default)]
pub(super) struct Restrictions(BTreeSet<SearchRestriction>);

impl Restrictions {
    pub(super) fn insert(&mut self, restriction: SearchRestriction) {
        self.0.insert(restriction);
    }

    pub(super) fn contains(&self, restriction: SearchRestriction) -> bool {
        self.0.contains(&restriction)
    }

    pub(super) fn into_vec(self) -> Vec<SearchRestriction> {
        self.0.into_iter().collect()
    }
}

pub(super) fn ranked_indices<T>(items: &[T], compare: impl Fn(&T, &T) -> Ordering) -> Vec<usize> {
    let mut indices = (0..items.len()).collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        compare(&items[*right], &items[*left]).then_with(|| left.cmp(right))
    });
    indices
}

pub(super) fn ranked_indices_filtered<T>(
    items: &[T],
    include: impl Fn(&T) -> bool,
    compare: impl Fn(&T, &T) -> Ordering,
) -> Vec<usize> {
    let mut indices = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| include(item).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        compare(&items[*right], &items[*left]).then_with(|| left.cmp(right))
    });
    indices
}

pub(super) fn quota_union(lanes: &[(&Vec<usize>, usize)], limit: usize) -> Vec<usize> {
    let mut selected = Vec::with_capacity(limit);
    let mut seen = BTreeSet::new();
    for (lane, quota) in lanes {
        for index in lane.iter().take(*quota) {
            if seen.insert(*index) && selected.len() < limit {
                selected.push(*index);
            }
        }
    }
    let mut offset = 0usize;
    while selected.len() < limit {
        let mut visited = false;
        let mut added = false;
        for (lane, _) in lanes {
            if let Some(index) = lane.get(offset) {
                visited = true;
                if seen.insert(*index) {
                    selected.push(*index);
                    added = true;
                    if selected.len() == limit {
                        break;
                    }
                }
            }
        }
        if !visited {
            break;
        }
        offset += 1;
        if !added && lanes.iter().all(|(lane, _)| offset >= lane.len()) {
            break;
        }
    }
    selected
}

pub(super) fn diversify<T>(
    items: &[T],
    ranked: Vec<usize>,
    first_key: impl Fn(&T) -> u64,
) -> Vec<usize> {
    let mut grouped = std::collections::BTreeMap::<u64, Vec<usize>>::new();
    let mut first_seen = Vec::new();
    for index in ranked {
        let first = first_key(&items[index]);
        if !grouped.contains_key(&first) {
            first_seen.push(first);
        }
        grouped.entry(first).or_default().push(index);
    }
    let mut diversified = Vec::new();
    let mut round = 0usize;
    loop {
        let mut added = false;
        for first in &first_seen {
            if let Some(index) = grouped.get(first).and_then(|group| group.get(round)) {
                diversified.push(*index);
                added = true;
            }
        }
        if !added {
            break;
        }
        round += 1;
    }
    diversified
}

pub(super) fn take_selected<T>(items: Vec<T>, selected: &[usize]) -> Vec<T> {
    let mut items = items.into_iter().map(Some).collect::<Vec<_>>();
    selected
        .iter()
        .filter_map(|index| items[*index].take())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrictions_are_deduplicated_and_emitted_in_canonical_order() {
        let mut restrictions = Restrictions::default();
        restrictions.insert(SearchRestriction::StateBudget);
        restrictions.insert(SearchRestriction::ProbePool);
        restrictions.insert(SearchRestriction::FirstStepShortlist);
        restrictions.insert(SearchRestriction::ProbePool);

        assert_eq!(
            restrictions
                .into_vec()
                .into_iter()
                .map(SearchRestriction::as_str)
                .collect::<Vec<_>>(),
            vec!["first_step_shortlist", "probe_pool", "state_budget"]
        );
    }

    #[test]
    fn diversity_takes_one_state_per_first_issue_before_second_variants() {
        let states = [(1, "a"), (1, "b"), (2, "a"), (2, "b"), (3, "a")];

        let diversified = diversify(&states, vec![0, 1, 2, 3, 4], |state| state.0);

        assert_eq!(diversified, vec![0, 2, 4, 1, 3]);
    }
}
