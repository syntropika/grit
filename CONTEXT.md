# Hyfa

Hyfa analyzes work recorded in GitHub as a graph to identify blockers and recommend what to tackle next.

## Language

**Issue**:
The canonical unit of work persisted in GitHub and the basic node analyzed by Hyfa.
_Avoid_: Hyfa task, bead

**Dependency**:
A directed relationship between a dependent Issue and another Issue that must be resolved before it.
_Avoid_: Blocking label, related link

**Issue graph**:
The set of analyzed Issues and their Dependencies, considered as a directed structure.
_Avoid_: Project graph, Beads graph

**Operational graph**:
The subset of the Issue graph formed by open Issues and still-unsatisfied Dependencies, used to decide current work. A closed Issue may satisfy a Dependency, but it does not belong to this graph.
_Avoid_: Historical graph, complete backlog

**Ready Issue**:
An open Issue whose Dependencies are satisfied and that has no open or unknown-state External blocker. Only Ready Issues may be recommended as next work.
_Avoid_: Important Issue, almost-unblocked Issue

**Available Issue**:
A Ready Issue with no assignee and therefore available as new work. Assignment affects its availability, not its Dependencies or readiness.
_Avoid_: Ready Issue, unblocked Issue

**Execution scope**:
The rule that determines which Ready Issues Hyfa may propose as steps in a calculation. It combines availability or an explicit assignee with optional required labels, excluded labels, and direct children of a Parent Issue; it does not remove blockers or limit which unlocked outcomes count.
_Avoid_: Graph scope, readiness, global availability

**Executable Issue**:
A Ready Issue that belongs to the active Execution scope. Steps in `next` and rollouts come from this set; it may be an Available Issue or work belonging to an explicitly requested assignee.
_Avoid_: Available Issue, any Ready Issue, important Issue

**Planning horizon**:
The limit on future work that Hyfa simulates when comparing possible next steps. Anything beyond it is treated as downstream potential, not as part of the evaluated plan.
_Avoid_: Complete plan, whole-backlog optimization

**Dependency layer**:
A counterfactual topological layer of the Graph scope under infinite capacity: layer 0 contains the current Ready Issues, and each later layer contains Issues that would become Ready after resolving all blockers in earlier layers. It carries assignment and Execution-scope annotations, but does not represent time, team barriers, assignments, or a parallel recommendation.
_Avoid_: Sprint, ETA, worker wave, execution plan

**Unlock set**:
The set of distinct Issues that transition from blocked to Ready for the first time when a plan is completed within the Planning horizon. It includes assigned outcomes and respects AND dependencies without duplicating diamonds.
_Avoid_: Completed Issues, open descendants, fan-out

**Unlock profile**:
The ordered description of an Unlock set by count, Declared priority composition, and when its members become Ready. It is not a decimal score and makes no claim about business value.
_Avoid_: Unlock score, PageRank, impact, raw descendant count

**Unlock curve**:
The cumulative number of Unlock set members after each step in a plan. It distinguishes plans with the same final result when one enables work earlier.
_Avoid_: Timeline, ETA, Critical distance

**Declared priority**:
The ordinal signal explicitly assigned by a person to an Issue through exactly one canonical `priority:p0`–`priority:p4` label, where P0 is the highest priority. It is the only manually supplied ranking input; it neither replaces the Unlock profile nor implies impact or effort.
_Avoid_: Unlock profile, impact, effort, calculated score

**Priority propagation**:
The incorporation of the Declared priority of downstream outcomes that actually enter a plan's Unlock set. It does not change labels, cause a blocker to inherit priority, or reach descendants that remain blocked at the end of the Planning horizon.
_Avoid_: Copying labels, ordering only by the first step's priority, persisted transitive priority

**Critical priority**:
Declared priority P0, reserved as an expedite class. An Executable P0 has absolute precedence; when blocked, that precedence applies only to an executable plan that actually makes it Ready within the Planning horizon, not to any remote ancestor.
_Avoid_: P1, any important Issue, P0 descendant outside the horizon

**Critical distance**:
The minimum number of distinct Issues that must be completed from the current state to make a P0 Issue Ready while satisfying all its AND Dependencies. It measures units of graph work, not time or effort.
_Avoid_: Edge count along a single path, estimated duration, distance to any descendant

**P0 unlock curve**:
The cumulative number of P0 Issues in the Unlock set after each rollout step. Within P0 mode it is compared before the normal Unlock profile to favor enabling more critical work and enabling it earlier.
_Avoid_: Critical distance, total open P0 count, priority score

**Priority conflict**:
The invalid state of an Issue containing more than one canonical `priority:*` label. Hyfa reports the conflict and ignores that Issue's Declared priority, but does not change its readiness or eligibility for calculated ranking.
_Avoid_: Dependency blocker, Mutation conflict, highest priority among the present labels

**Priority update**:
The logical mutation that sets an Issue's Declared priority to one P0–P4 value or none. Although represented through labels, it replaces only the canonical `priority:*` subset and preserves every other label.
_Avoid_: Replacing all labels, adding a second priority, Issue Field update

**Repository initialization**:
The explicit, idempotent preparation performed by `hyfa init` on a Repository. In v1 it creates any missing canonical `priority:p0`–`priority:p4` labels without renaming, deleting, or modifying existing labels and without assigning them to an Issue.
_Avoid_: Synchronization, analysis with side effects, destructive label migration

**Structural centrality**:
A measure of topological position and influence, such as PageRank. It describes structure, not readiness, Unlock profile, or operational priority.
_Avoid_: Operational priority, Unlock profile

**Dependency impact**:
An explanation of the open work that depends on an Issue, distinguishing its direct and transitive dependents from the work that would become Ready after its completion. Immediate completion outcomes apply only to Ready Issues, independently of ownership or Execution scope.
_Avoid_: Business value, ranking score, Unlock profile

**Downstream chain depth**:
The largest number of Dependencies in an open chain leading from an Issue to its downstream dependents. It describes structural depth, excludes closed history, and is unavailable when a reachable cycle prevents a finite result.
_Avoid_: Critical path, ETA, Critical distance, Dependency layer

**Local replica**:
A local, disposable, rebuildable representation of a Repository's Issues and Dependencies, used as analysis input. GitHub remains authoritative for every change, and the replica never acts as a source of truth.
_Avoid_: Local backlog, authoritative copy, Issue database

**Synchronization**:
The operation that attempts to obtain a new, complete GitHub state with which to replace the valid Local replica. Analysis commands attempt it before calculating and, when GitHub is unavailable, continue from the latest valid replica while reporting its `synced_at`.
_Avoid_: Bidirectional synchronization, authoritative replica

**Synchronization time**:
The instant of the last complete Synchronization, exposed as `synced_at`. It reports when the Local replica was published, but it is not an API cursor and is not used directly to prevent missed changes.
_Avoid_: Age, watermark, freshness guarantee

**Full reconciliation**:
A complete Synchronization that rebuilds and compares current state when Hyfa cannot prove continuity of an incremental update.
_Avoid_: Ordinary update, manual JSON repair

**Pending mutation**:
A locally persisted intent to change GitHub that GitHub has not yet accepted or recognized as already satisfied. It is stored separately from the Local replica and never turns that replica into a source of truth.
_Avoid_: Local Issue edit, synchronized change, local Issue

**Mutation reconciliation**:
The process of first retrieving current GitHub state, revalidating and applying Pending mutations, halting conflicting operations, and synchronizing the Local replica again.
_Avoid_: Last-write-wins, merge of two authoritative copies

**Working graph**:
The provisional view obtained by applying ordered Pending mutations over the Local replica, used for offline analysis. Differences not yet accepted by GitHub are always identified as `pending`.
_Avoid_: Local replica, synchronized graph, source of truth

**Draft Issue**:
An Issue created locally that does not yet exist in GitHub. It participates provisionally in the Working graph and may receive content, comments, and relationships through Pending mutations.
_Avoid_: Synchronized Issue, GitHub draft, private Issue

**Temporary Issue ID**:
The stable, opaque local identity of a Draft Issue, used by other Pending mutations until GitHub assigns its canonical ID and number. Its format remains outside the contract for now.
_Avoid_: Issue number, GitHub ID, queue position

**Stable node key**:
The total, reproducible key used only for traversals and final tie-breaking: `(0, issue_number)` for a GitHub Issue and `(1, temporary_id_bytes)` for a Draft Issue. It represents neither priority nor age and may change class when a draft receives a remote identity, together with a new `input_hash`.
_Avoid_: Declared priority, ranking score, creation date

**Operation marker**:
A random, non-secret identifier embedded as an invisible HTML comment in an Issue or comment created from a Pending mutation, allowing Hyfa to recognize a remote write whose outcome became uncertain. Hyfa removes it from all public and user-facing output.
_Avoid_: User content, token, GitHub ID

**Mutation conflict**:
A Pending mutation whose relevant remote value no longer matches its base or desired result, preventing Hyfa from applying it without choosing between concurrent intentions.
_Avoid_: Network failure, already-satisfied change, last-write-wins

**Conflict resolution**:
The explicit, auditable decision that resolves a Mutation conflict by keeping the remote value, reaffirming the local value, or replacing both with a new value. Reaffirming or replacing updates the base to the observed remote value and rechecks the mutation before writing.
_Avoid_: Automatic resolution, silent last-write-wins, blind retry

**Repository**:
The GitHub repository whose work forms the boundary of a Hyfa analysis; every analysis belongs to exactly one.
_Avoid_: Project, workspace, organization

**Graph scope**:
The Issues belonging to an analysis's Repository, regardless of how they are later filtered for a particular calculation.
_Avoid_: Project membership, organization backlog

**Parent relationship**:
A GitHub relationship that groups a child Issue under a Parent Issue for decomposition. Direct-child membership can constrain the Execution scope, but is not a Dependency and does not change readiness.
_Avoid_: Dependency, implicit blocker, recursive membership

**External blocker**:
An Issue from another Repository that blocks an Issue in the Graph scope; it affects readiness but does not enter the scope or receive a ranking.
_Avoid_: In-scope Issue, ignored dependency

**Project**:
An optional GitHub grouping for planning and presenting Issues; it neither defines the truth of the Issue graph nor is required to analyze it.
_Avoid_: Canonical backlog, Hyfa database
