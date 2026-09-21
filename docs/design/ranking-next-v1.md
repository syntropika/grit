# `next/v1` ranking and planning

**Status:** normative Grit v1 contract  
**Decision:** ADR 0028  
**Scope:** one Repository, its Operational graph, open Issues, and unsatisfied GitHub-native Dependencies

## Objective

`grit next` must return executable work and explain why it is the best first step found. The policy favors unlocking real work, respects human priority without turning Grit into a label sorter, and keeps cost predictable with thousands of Issues.

There is no universal decimal score. `next/v1` uses an ordered key made of observable measures. Every comparison is explained by showing the first component on which two alternatives differ.

## Invariants

1. A recommendation is always an Executable Issue within the active Execution scope.
2. Priority, PageRank, or any other signal can never override an open or unknown Dependency or membership in a cyclic SCC.
3. Every rollout completes only Issues that are executable after its previous steps.
4. Dependencies use AND semantics: a dependent becomes Ready only when all its blockers are satisfied.
5. Every unlocked Issue is counted once, even in diamonds or when reached through multiple paths.
6. A closed Issue or closed historical chain cannot alter operational ranking.
7. Missing metrics are omitted; they are never replaced with a maximum or uniform contribution.
8. Given the same input, configuration, and deterministic budget, ordering is reproducible. An outer wall-clock cancellation aborts the command instead of returning a load-dependent recommendation.
9. The mode is recalculated before every simulated step: a rollout never postpones an Executable P0 to continue non-critical work.

## Graph preparation

For every snapshot, Grit:

1. Builds the Operational graph from open Issues and unsatisfied Dependencies.
2. Deduplicates edges.
3. Calculates each Issue's number of open blockers.
4. Detects strongly connected components in `O(V+E)`.
5. Marks cyclic Issues as not Ready and keeps their descendants blocked.
6. Retains External blockers as an opaque boundary: verifiably closed satisfies the dependency; open or unknown blocks it.
7. Builds blocker → dependent and dependent → blocker indexes.

For PageRank, Grit condenses the graph by component before calculation and deduplicates edges between components again. Edge orientation propagates pressure from dependents toward blockers. `next/v1` fixes damping at `0.85`, starts from a uniform vector, redistributes dangling mass uniformly, and performs 20 power iterations. It uses `binary64` and accumulates nodes and edges by Stable node key so quantization remains stable. The tie-break uses the integer bucket `floor(score * 1_000_000)`, not the raw `float`. An executable candidate can never belong to a cyclic SCC, so it directly uses the score of its singleton component; SCC scores do not need to be distributed among candidates. Betweenness is not part of the `next/v1` hot path.

## Candidate gate

The default Execution scope selects Issues without an assignee. Within that scope, candidates are open Issues that simultaneously:

- are Ready;
- are not cyclic;
- have no open or unknown External blocker;
- have no assignee;
- belong to the analyzed Repository.

An explicit assignee filter changes the Execution scope to Ready Issues assigned to that person. Issues assigned outside the filter are not simulated as steps. In either case, a candidate that passes the gate is an Executable Issue.

Assignment does not prevent an Issue from being counted in the Unlock set: making already-committed work Ready is still a graph outcome. Execution scope selects who can execute a step; readiness describes which work becomes enabled.

If there are no candidates, the command succeeds with `recommendation: null` and a summary of how many Issues are blocked, assigned, cyclic, or affected by unknown state. A sync failure without a valid Local replica remains an error.

## Priority interpretation

The only human-supplied ranking input is one canonical `priority:p0`–`priority:p4` label.

- P0: Critical priority and a separate expedite mode.
- P1: high.
- P2: normal.
- No label: neutral, equivalent to P2 for comparison while preserved as `unspecified` in the explanation.
- P3: low.
- P4: minimum.
- Priority conflict: reported as a warning and compared as neutral because its declared priority is ignored.

P1–P4 are ordinal. Grit does not invent cardinal distances such as “P1 is worth four times P3.”

## Rollout and Unlock profile

A rollout is a simulated sequence of up to `horizon` completions. The Ready frontier is recalculated after every step.

Before selecting each step, Grit reruns the gate with the remaining horizon and selects one of three modes: Executable P0, qualifying P0 route, or normal. This preserves P0 precedence within the rollout's fixed budget; it does not claim to match separate new invocations of `next`, which would reset the full horizon.

The **Unlock set** contains the distinct Issues that transition from blocked to Ready for the first time during the rollout. It does not contain an Issue merely because that Issue was completed, and it does not duplicate an Issue that is later completed within the same rollout.

The **Unlock profile** contains:

1. `count`: final Unlock set cardinality;
2. `priority_profile`: ordinal outcome distribution `[P1, neutral, P3, P4]`;
3. `curve`: cumulative cardinality after each step, for example `[1, 1, 7]`;
4. `step_priorities`: priorities of completed Issues in order, as a vector of length `horizon`; a rollout that ends early is padded with `no_step`, below P4.

`curve` is also padded to `horizon` by repeating its latest cumulative value, or with zero when no step occurred. P0 is omitted from `priority_profile` because P0 modes compare it first through `p0_curve`; `count` still includes every distinct transition to Ready. `p0_curve` is the cumulative number of P0 members in the Unlock set after each step and uses the same padding. `step_priorities` follows the order P0, P1, neutral, P3, P4, `no_step`. Neutral combines P2, missing priority, and Priority conflict for comparison, while JSON preserves their original states separately.

A downstream Issue's priority contributes only when the rollout actually makes that Issue Ready within the Planning horizon. A distant descendant is shown as potential, not reported as an unlock.

## P0 mode

Mode selection occurs before normal ranking.

### P0 already executable

If any P0 is Executable, only those P0 Issues are first-step candidates; prerequisites of other blocked P0 Issues do not enter this frontier. Their rollouts are compared first by descending `p0_curve` and then by the complete normal key—`normal_outcome_key`, PageRank bucket, and Stable node key—defined below. Completing a P0 that enables another P0 therefore beats completing a P0 that enables only P1–P4 work.

### P0 blocked but reachable

When no P0 is executable, Grit calculates the transitive closure of open prerequisites required to make each blocked P0 Ready. A P0 qualifies for the expedite mode only when:

- its closure is acyclic;
- it contains no open or unknown External blocker;
- every step can become Executable within the Execution scope;
- its Critical distance does not exceed the Planning horizon.

Critical distance is the number of distinct Issues in the closure, not the length of a convenient path. This respects AND dependencies and shared nodes. In a realized critical rollout it equals the one-based position of the first non-zero entry in `p0_curve`; static closures qualify and discover routes but cannot bypass mode reevaluation in later states.

Critical plans are compared by:

1. shortest Critical distance to the first Ready P0;
2. best lexicographic `p0_curve`: more newly Ready P0 Issues, earlier;
3. best complete normal key for all remaining outcomes.

The curve subsumes the former tie-breaks “more P0 at the minimum distance” and “more total P0”: for example, `[0, 2, 2]` beats `[0, 1, 3]` because it enables more critical work earlier.

A P0 outside the horizon, behind a cycle, dependent on unknown state, or requiring assigned work outside the scope does not receive absolute precedence. `triage` may still display it as critically blocked.

## Normal mode

The `normal_outcome_key` contains no structural metrics or IDs:

```text
(
  unlock_count,
  unlock_priority_profile[P1, neutral, P3, P4],
  unlock_curve[step 1 ... horizon],
  step_priority_sequence
)
```

When P0 mode does not apply, two rollouts are compared lexicographically and descending, adding only the final tie-breaks:

```text
(
  normal_outcome_key,
  first_step_pagerank_bucket,
  first_stable_node_key ascending
)
```

Deliberate consequences:

- Three P4 unlocks beat two P1 unlocks: a larger Unlock set can compensate for Priority.
- With the same unlock count, outcomes containing P1 beat P2/P3/P4 outcomes.
- With the same result and Priority, unlocking earlier is better.
- With no unlocks, the step's own priority decides before PageRank.
- PageRank never overrides a difference in Unlock profile or Priority; it only breaks a structural tie.
- Stable node key is the final fallback and is not presented as a signal of age or value.

`downstream_open_reach`, depth, and PageRank may appear as diagnostics or help form the shortlist, but they are not added to the Unlock profile again. This avoids counting the same topology multiple times.

`unlock_count` inevitably depends on how granularly a Repository divides its work. V1 exposes this limitation and does not attempt to correct it with hidden weights; Priority comparison and explanations make the effect visible, and a future value signal will be added only as explicit data.

## Bounded search

Default `next/v1` values:

| Limit | Value |
| --- | ---: |
| Planning horizon | 3 completions |
| First-step shortlist | 96 Issues |
| Beam width | 64 states |
| Branch width | 32 steps per state |
| Joint probe | beam 8, branch 8, 64 successors |
| Probe work budget | 262,144 successors per command |
| Deterministic budget | 8,192 materialized successors |

With horizon 1, Grit evaluates every candidate exactly using blocker counters in `O(V+E)`.

For larger horizons, the shortlist is the deterministic union of:

- up to 32 candidates by immediate unlocks;
- up to 16 by structural downstream reach within the horizon;
- up to 16 by a feasible joint probe within the horizon;
- up to 16 by Declared priority;
- up to 16 by PageRank.

Structural reach is only a discovery heuristic: the number of distinct open descendants reachable within at most `horizon` edges. AND-feasible potential used as an upper bound is stricter: from a state with `remaining_steps`, it includes a blocked outcome only when its known open prerequisite closure is acyclic, has no opaque boundary, individually fits within those steps, and is executable within the Execution scope. It deduplicates the closure but remains deliberately optimistic because multiple outcomes may compete for the same steps.

The joint probe compensates for that overestimation with a real mini-rollout anchored to the first candidate being scored. In normal mode it continues only through the candidate's causal cone—open descendants and every open co-blocker required by AND—preventing a mediocre task from taking credit for choosing completely unrelated work afterward. The cone extends after each step with dependents whose blocker count changed and their co-blockers; when the dynamic gate detects P0, its critical steps replace this restriction.

Inside the probe, branch 8 and beam 8 are divided into `2 causal / 3 realized / 3 upper`. The causal lane orders continuation categories as follows: first, an Issue made Ready by the anchored rollout; second, an Executable co-blocker of an affected dependent. Within a category it uses the realized key and then Stable node key. For states it first prefers the longest sequence of newly Ready causal continuations and then the realized key. When a causal continuation exists, at least one survives every level. The probe materializes at most 64 successors, does not invoke itself recursively, and returns the key of the best actually simulated rollout; realized and upper decide what else to explore but do not become outcomes.

Probes are never run over an unbounded frontier. A cheap deterministic pool is formed before the joint quota:

- initial probe pool, maximum 256: 128 by immediate result, 64 by structural reach, 32 by Priority, and 32 by PageRank;
- branch, maximum 32: 8 global realized, 8 global upper, 8 realized within the causal cone, and 8 upper within the causal cone;
- beam, maximum 64: 32 lane-diverse states by realized result and 32 by upper.

The pool is deduplicated and filled round-robin in the order above; only then is the probe run and the joint quota selected. Trimming any of these pools declares `probe_pool`. Every probe has a local limit of 64, and all probes share a deterministic `probe_work_budget` of 262,144 successors per command. They are evaluated in stable pool order; after the budget is exhausted, remaining candidates receive no joint key but continue competing in the other lanes, and `probe_budget` is declared.

These discovery metrics are never presented as an Unlock set. Initial construction visits at most 1,024 nodes per candidate, and per-state estimation visits at most 512; small closures are memoized by input and remaining steps. Reaching either limit is recorded as `potential_budget`.

The separate P0 frontier depends on the active mode: it contains only Executable P0 Issues when any exist; otherwise it contains Ready prerequisites belonging to the closure of a qualifying blocked P0. It does not compete for normal quotas, and every member receives an exact one-step evaluation. For horizons greater than 1, if this frontier exceeds 96 first steps, it is ordered by the smallest associated critical closure, the largest number of qualifying P0 closures containing the step, best potential `p0_curve`, best immediate result, PageRank, and Stable node key; it keeps 96 and declares `p0_frontier`. Precedence remains restricted to that frontier, but the result does not claim to be its global optimum.

After the normal shortlist is deduplicated, gaps are filled using the same cheap stable key. The initial state expands every first step in the shortlist, up to 96. At later depths, the 32 branches are the union of up to 16 steps by immediate result, 8 by joint probe, and 8 by AND upper bound; gaps are filled by alternating the three keys stably.

The beam does not base pruning solely on already-realized outcomes. It keeps up to 32 states by the active mode's realized key, 16 by the joint probe's best feasible outcome, and 16 by an optimistic key made from `p0_curve`, the current Unlock profile, and the remaining AND upper bound. Within each lane it groups by first Issue and takes the best state from each group in rounds before accepting a second state with the same first step. After deduplication it fills by alternating the three lists. This reservation protects delayed cascades, prevents variants of one candidate from monopolizing the beam, and preserves an upper bound for AND complementarity.

### Deterministic pruning keys

Every list is ordered descending except for its final fallback, which uses ascending Stable node key or, for states, the complete ascending sequence of those keys. The keys are:

- immediate quota: exact one-step `normal_outcome_key`, PageRank, Stable node key;
- structural quota: count and Priority profile of unique structural reach, immediate result, PageRank, Stable node key;
- joint quota: probe's best feasible key, immediate result, PageRank, Stable node key;
- Priority quota: step Priority, immediate result, PageRank, Stable node key;
- PageRank quota: PageRank bucket, immediate result, Stable node key;
- realized branch and realized beam: active mode's complete key over the partial state;
- joint branch and joint beam: probe's best feasible key, realized key, sequence;
- upper branch and upper beam: optimistic key, realized key, sequence.

For a state at depth `d`, every AND-potential outcome retains the minimum size of its individual open closure. The optimistic curve keeps the real curve for the first `d` steps and, for every future step `d+k`, adds every potential outcome whose closure fits in `k`; it does the same separately for P0. The descending optimistic key is:

```text
(
  optimistic_p0_curve,        # only in P0 mode
  optimistic_unlock_count,
  optimistic_priority_profile[P1, neutral, P3, P4],
  optimistic_unlock_curve,
  realized_partial_key,
  full_stable_node_key_sequence ascending
)
```

This is an individual upper bound, not a promise that all those outcomes fit together. It resolves cases such as three P4 outcomes versus two P1 outcomes with the same semantics as real ranking while reserving beam capacity for a cascade that has not yet materialized.

Formally, if `U` is the state's real Unlock set and `F_k` contains distinct potential outcomes with a minimum closure `<= k`, the future curve point `d+k` is `|U ∪ F_k|`; the optimistic profile is calculated over `U ∪ F_{remaining_steps}`, with P0 separated into its own curve. No member is counted twice.

Every group contributes its quota first in the order shown in this contract. Gaps caused by duplicates are filled by round-robin traversal of the complete lists in the same order. Every discovery BFS visits nodes by ascending `(distance, Stable node key)` and counts a node when it is removed from the queue for the first time. A memoized result consumes the same number of logical visits as a cold calculation, so warm and cold caches produce the same pruning and flags.

When PageRank is globally omitted, its quota is empty and round-robin fills those positions from the other four lists.

A state consumes `state_budget` when the main search materializes a successor by appending one executable step; the root does not count. Successors internal to a probe consume only the probe's local and global budgets. Both counters follow the deterministic orders above and are checked before materializing the next successor. Cache state, allocation, and memory therefore cannot alter what work fits within either budget.

The deterministic state and probe budgets bound ranking work. Wall-clock cancellation belongs outside the ranking engine and aborts the command rather than changing the explored set or returning a load-dependent recommendation. Grit records every discard caused by shortlist, P0 frontier, downstream metric limit, branch, beam, or state. The result remains executable, but search is declared complete only when no candidate or state was discarded:

```json
{
  "search_complete": false,
  "truncated_by": ["first_step_shortlist", "beam_width"],
  "global_optimum_claimed": false
}
```

`truncated_by` is an ordered list without duplicates. Its v1 values, in canonical order, are `first_step_shortlist`, `potential_budget`, `probe_pool`, `probe_budget`, `p0_frontier`, `branch_width`, `beam_width`, and `state_budget`. With horizon 1, shortlist, probe, limited P0 frontier, beam, and branch do not apply: every candidate is evaluated exactly.

Ranking is recalculated from the effective input and cached by `input_hash`, policy version, and parameters. `input_hash` covers the Local replica snapshot, the ordered Pending mutation overlay, and the Execution scope; two different Working graphs never share a result merely because they have the same `synced_at`. No hidden incremental score is maintained.

## `next`, `ready`, and `plan`

### `grit next`

Returns one recommendation, the best rollout that justifies it, and alternatives. Before comparing candidates, Grit keeps only the best explored rollout for each first Issue; alternatives and the runner-up always have a different first step. A complete tie between rollouts for the same candidate is resolved by the ascending full sequence of Stable node keys, with `no_step` last. It is read-only: it attempts a pull sync but never publishes Pending mutations.

### `grit ready`

Returns the complete executable frontier. Its purpose is to enumerate state, not promise that its first item is a recommendation; default ordering is deterministic by Stable node key.

### `grit plan`

Without explicit capacity, it returns:

- the best single-lane rollout;
- `parallel_now`, the complete frontier of Executable Issues in the active scope;
- `dependency_layers`, counterfactual topological layers rather than dates.

Given the same input and parameters, that lane's first step and explanation are exactly those of `grit next`; `plan` does not maintain an alternative ranking.

`dependency_layers` applies an infinite-capacity structural counterfactual over the entire Graph scope, not only the Execution scope:

- `layer 0` contains all current Ready Issues and annotates which are Executable;
- for an open acyclic Issue without an opaque external boundary, `layer(issue) = 1 + max(layer(blocker))` over all its open internal blockers;
- an Issue without open internal blockers already belongs to `layer 0`;
- membership in an SCC, an open or unknown External blocker, or any blocker without a finite layer places the Issue in `unresolved`, without a layer number.

Because Dependencies use AND semantics, an Issue appears only after the last layer of all its blockers. A layer may contain assigned work or work outside the Execution scope and marks it as such; this explains topology but does not authorize Grit to recommend it as a step.

Grit v1 does not accept `--workers N`. Selecting sets introduces another decision function—overlap, wide P0 closures, each worker's skills, and parallel rounds—that cannot honestly be derived from `next/v1`. `parallel_now` exposes all immediate capacity, and `dependency_layers` shows only the structural counterfactual; neither represents barriers or assignments.

A future `plan/v2` may add joint capacity under the invariant `workers=1 == next`, `critical_rounds` for P0, and its own budgets. Without Effort or temporal capacity, Grit v1 does not call these layers `critical_path`, an ETA, or a calendar. It may display `remaining_dependency_depth`, but it does not pretend to know duration.

## Explanation and JSON

Output includes `policy_version: "next/v1"`, `replica_snapshot_hash`, `input_hash`, `synced_at`, scope, mode, parameters, every metric's state, and the first component that decided against the runner-up. It also exposes deterministic `work.materialized_successors` and `work.probed_successors`; unlike elapsed time, these counts must be identical on cold and warm runs. Every rollout step includes the recalculated mode that allowed it to be selected. When a local overlay exists, output also includes `pending: true` and the identifiers of operations that affected the result. When pruning occurred, `runner_up_scope` is `explored`; only a complete search may use `global`.

Minimum reason codes:

- `ready_p0`;
- `p0_gate_continues`;
- `shortest_p0_route`;
- `unlocks_more_p0`;
- `shared_p0_prerequisite`;
- `unlocks_more_work`;
- `unlocks_higher_priority_work`;
- `unlocks_earlier`;
- `declared_priority_tiebreak`;
- `pagerank_tiebreak`;
- `deterministic_tiebreak`.

Output marks `close_call: true` when the two best distinct first Issues differ only by PageRank or Stable node key. It does not present a `0.731` versus `0.729` difference as scheduling certainty.

`comparison_to_runner_up` is the canonical typed decisive reason whenever a distinct runner-up exists and carries its human presentation message. The recommendation's `reasons` array contains only mode-specific supporting facts, each with its canonical message; consumers must not infer the decisive comparison from array position. With one candidate, `only_executable_candidate` is the decisive mode reason and its serialized message is the presentation source.

PageRank is calculated with the fixed parameters above, and output exposes both its state and `pagerank_bucket`. It is a global stage: either every candidate receives a bucket or PageRank disappears from every key. If it fails, expires, or is omitted, the stable fallback is used; it is never replaced with uniform values or missing for only some nodes.

Results based on a Working graph mark affected edges, Issues, and reasons as `pending`. A Priority conflict appears in `warnings` even though the Issue remains eligible with neutral priority. The explanation breaks down assigned and unassigned members of the Unlock set even though both count equally in v1.

## Scale cost and budget

Preparation, readiness, SCC, and immediate unlocks cost `O(V+E)`. PageRank costs `O(I(V+E))`. Upper-bound construction is capped at `1,024 × candidates` in the shortlist and `512 × evaluated states` during search, plus feasible closures with maximum size `horizon + 1`. Outcomes that have no feasible bounded closure are excluded from upper-bound iteration. No probe exceeds 64 successors, and all probes together remain below 262,144. The combinatorial portion depends on these limits, shortlist, beam, branch, and state budgets rather than enumerating every possible plan.

Grit persists the complete PageRank and bounded-search result as a disposable cache. Its key includes the effective Working-input hash, policy version, Planning horizon, alternative and state budgets, and PageRank parameters. The Working-input hash includes the replica snapshot, ordered Pending overlay, and normalized Execution scope. A cache hit still rebuilds and validates the Operational graph before rehydrating Issue references; malformed, stale, or semantically incompatible cache data is a miss. Cache hits never change deterministic work counts, result hashes, alternatives, or truncation reasons.

The reproducible release benchmark uses 5,000 Issues, 20,000 distinct Dependencies, horizon 3, mixed feasible and infeasible AND closures, the complete normative probe/search policy, output assembly, cache publication or validated lookup, and analysis JSON serialization. Synchronization and fixture construction are outside the measurement. On the documented reference VM, five runs had a median cold time of about 662 ms and a median warm time of about 45 ms. The enforced gates remain below 1 s cold and below 250 ms warm. Exact hardware, fixture construction, phase results, and the command are recorded in `docs/performance/next-v1.md`.

Exact betweenness, enumeration of every cycle, and unbounded transitive reach remain outside the hot path.

## Required scenarios

At minimum, the policy must preserve these behaviors:

- a chain with a cascade inside and outside the horizon;
- a delayed cascade that must survive the beam against 65 immediate-outcome alternatives;
- fan-out converging through a diamond, with outcomes deduplicated;
- `A + B → C` with an AND dependency;
- Ready P0, reachable P0, and P0 outside the horizon;
- P0 enabling another P0 versus P0 enabling P4, plus timing across multiple P0 Issues;
- reevaluation of the P0 gate at every rollout step;
- two P0 routes with different Critical distances;
- a prerequisite shared by multiple P0 Issues;
- more low-priority unlocks versus fewer high-priority unlocks;
- a graph tie resolved by Priority;
- missing Priority and Priority conflict;
- an assigned candidate and an assigned downstream outcome;
- an SCC, plus open, closed, and unknown External blockers;
- irrelevant closed history;
- missing PageRank;
- complete and truncated search;
- a feasible cascade against 64 decoys with mutually incompatible AND upper bounds;
- `plan` without capacity and explicit rejection of `--workers` in v1;
- deterministic ordering and JSON.

The prototype in `prototypes/ranking_v1/` validates the semantic core with 20 small fixtures and measures the first diverse-pruning implementation, including the cascade regression. The remaining scenarios in this list are mandatory implementation acceptance tests; the prototype is not a complete validation of the contract.
