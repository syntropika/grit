# PROTOTYPE — `next/v1` ranking

Question: does a weight-free, bounded policy choose useful first steps across dependency chains, AND dependencies, fan-out, missing priorities, and critical P0 work without reducing Grit to a priority-label sorter?

This is throwaway Python code for validating the decision model. It has no persistence and does not call GitHub.

`ranking.py` is an exhaustive oracle only for its tiny, horizon-bounded fixtures. It models default unassigned availability, opaque missing blockers, neutral Priority conflicts and `recommendation: null`; it intentionally omits PageRank, assignee-filter scopes, synchronization, warnings and the final JSON contract. Omitting PageRank also exercises the required stable fallback path.

Run every built-in scenario:

```bash
python3 prototypes/ranking_v1/tui.py --all
```

Drive the scenarios interactively:

```bash
python3 prototypes/ranking_v1/tui.py
```

## Verdict

The selected lexicographic policy behaved as intended across 20 built-in chain, fan-out, AND, P0, priority, assignment, opaque-blocker, no-candidate, and cycle scenarios. It lets a larger Unlock set compensate for differences among P1–P4 priorities, reapplies the P0 gate at every simulated step, uses downstream Priority only after real readiness transitions, and confines P0 dominance to work reachable within the horizon.

The bounded-search benchmark is intentionally a Python reference rather than a production implementation:

```bash
python3 prototypes/ranking_v1/benchmark.py
```

On the development machine, the median of five runs for the 5,000-node/21,518-edge fixture was roughly 151 ms for horizon-3 search and 31 ms for 20 PageRank iterations after evaluating up to 96 initial candidates. The benchmark also contains a regression in which diversity-aware beam pruning preserves a 101-outcome delayed cascade that immediate-only pruning discarded. It models beam/branch cost with relaxed structural reach; it does not implement the complete shortlist quotas, AND-feasible potential, P0 modes, PageRank in the ranking key, state/time truncation, persistence, `plan`, or JSON explanations. The core decisions exercised here are captured in `docs/design/ranking-next-v1.md` and ADR 0028; the normative contract requires additional acceptance tests. This workspace is not a Git repository, so the throwaway prototype cannot be preserved on the separate branch normally required by the skill; it remains clearly isolated here instead.
