# Grit

Grit treats the Issues and native Dependencies in one GitHub Repository as a
graph. GitHub remains the source of truth; Grit keeps a disposable Local
replica so later analysis can be fast and work offline.

The current executable tracer implements full synchronization and Executable
frontier enumeration from
[Issue #2](https://github.com/syntropika/grit/issues/2) and
[Issue #3](https://github.com/syntropika/grit/issues/3), plus canonical
Declared priority support from
[Issue #6](https://github.com/syntropika/grit/issues/6), online Priority
updates from [Issue #9](https://github.com/syntropika/grit/issues/9), step-by-step
recommendations from [Issue #11](https://github.com/syntropika/grit/issues/11) and
[Issue #14](https://github.com/syntropika/grit/issues/14), offline Priority
intent projection from [Issue #15](https://github.com/syntropika/grit/issues/15),
and structural plans from [Issue #23](https://github.com/syntropika/grit/issues/23).

## Build and test

Grit requires a stable Rust toolchain with Edition 2024 support.

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## Synchronize a Repository

Grit first tries the token from the active `gh` session and falls back to a
non-empty `GH_TOKEN`:

```bash
grit sync --repo OWNER/REPO
grit sync --repo OWNER/REPO --json
```

`--json` emits the versioned `grit.sync/v1` result envelope with Repository
scope, `synced_at`, a deterministic input hash, and normalized entity counts.
Neither output mode includes credentials or raw GitHub responses.

The Local replica uses the operating system's application-data directory.
Set `GRIT_STATE_DIR` to isolate it, for example in CI. `GRIT_GITHUB_API_URL`
and `GRIT_GITHUB_HOST` support GitHub Enterprise and deterministic test
servers; the API URL must be a credential-free HTTP(S) base URL.

A synchronization retrieves every page of Issues and Repository Issue
comments, then the native `blocked_by` Dependencies for each Issue. Pull
Requests returned by the Issues endpoint are excluded. Grit writes only a
normalized model and atomically replaces the previous valid replica after the
entire load succeeds.

The replica file format and location below `GRIT_STATE_DIR` are implementation
details. Consumers should use Grit's versioned command output rather than read
the replica directly.

## Initialize Declared priority

Grit v1 reads Declared priority only from `priority:p0` through `priority:p4`
labels. Initialize missing labels explicitly:

```bash
grit init --repo OWNER/REPO
grit init --repo OWNER/REPO --json
```

Initialization creates only missing canonical names. It never renames,
recolors, redescribes, deletes, or assigns an existing label, so repeated runs
converge without further changes. Read commands never create labels.

Update one Issue's logical Priority with a full Issue reference:

```bash
grit update OWNER/REPO#NUMBER --priority p0
grit update OWNER/REPO#NUMBER --priority none --json
```

A concrete value removes every other canonical Priority label and leaves
exactly the requested one; `none` removes all canonical Priority labels. Grit
preserves non-Priority labels and every other Issue field. It writes GitHub
first, then synchronizes and verifies the logical result before publishing the
Local replica. If GitHub is unavailable before Grit can confirm the update,
Grit instead appends a versioned Pending mutation to a durable outbox. Each
operation retains the logical base and desired values; it never edits the Local
replica or advances `synced_at`. The output reports both the previous and
resulting Priority and whether the result is synchronized or pending.

## Enumerate Executable work

`grit ready` refreshes the Local replica and lists the complete Executable
frontier in ascending Issue-number order:

```bash
grit ready --repo OWNER/REPO
grit ready --repo OWNER/REPO --assignee LOGIN --json
```

Without `--assignee`, the Execution scope contains Ready unassigned Issues.
With it, the scope contains Ready Issues assigned to that login. Assignment is
reported separately from Dependency readiness. JSON reports every returned
Issue's priority as `declared`, `unspecified`, or `conflict`. A conflict remains
eligible and does not change readiness; it compares as neutral in later ranking
commands. Missing canonical Repository labels and conflicts appear as warnings.

Pending Priority mutations are applied in order over the Local replica to
form the Working graph used by `ready` and `next`. Their JSON identifies every
affected Issue and result with `pending: true` and the responsible operation
IDs. Analysis may pull newer GitHub state, but it never replays or writes the
outbox; reconciliation is a separate explicit operation.

If GitHub cannot be reached, `ready` uses the latest valid Local replica and
reports its unchanged `synced_at`. A replica is accepted only when its schema,
Repository scope, timestamp, and deterministic content hash validate. If no
valid replica exists, the command fails instead of inventing an empty graph.

## Recommend the next Issue

`grit next` evaluates rollouts of up to three Executable completions. Three is
the default Planning horizon; horizons one and two remain available:

```bash
grit next --repo OWNER/REPO
grit next --repo OWNER/REPO --horizon 1
grit next --repo OWNER/REPO --assignee LOGIN --horizon 3 --json
grit next --repo OWNER/REPO --profile --json
```

The `next/v1` policy first enforces Executable P0 and one-step P0-route gates.
Every later step is selected from the Executable frontier produced by its
predecessors. The policy compares distinct AND-aware transitions to Ready,
downstream Priority composition, the cumulative Unlock curve, the completed
Issues' Priority sequence, a quantized first-step PageRank tie-break, and
finally the Stable node key. Assigned outcomes count as unlocked work even
though only work inside the active Execution scope can be simulated as a step.
It never recommends blocked or out-of-scope work.

Horizon one remains an exact comparison of the complete first-step frontier.
For longer horizons, deterministic shortlist, probe, branch, beam, and state
budgets bound the search while preserving separate lanes for realized results,
feasible joint rollouts, and delayed cascades. Every activated restriction is
reported in `truncated_by`; a restricted result sets `search_complete` and
`global_optimum_claimed` to false and scopes the runner-up to `explored`.

Robot output reports both snapshot and effective-input hashes, metric states,
deterministic main-search and probe work counts, the runner-up comparison,
structured reasons, and whether the result is a close structural tie. Pending
recommendations, Issue references, comparison evidence, and reasons carry
operation provenance. If no
Issue is Executable, the command succeeds with a null recommendation and
categorized blocker counts. Like `ready`, it attempts a pull Synchronization
and falls back to the latest valid Local replica without mutating GitHub.

The ranking result is cached locally by effective input, policy version, and
all result-affecting parameters. The cache is disposable and never replaces
the Local replica or GitHub as the source of truth. `--profile` reports graph
preparation, SCC, readiness, cache, PageRank, bounded search, output assembly,
and analysis-serialization timings in microseconds; Synchronization is
explicitly excluded. Human profiling does not perform an unused JSON
serialization. See [`docs/performance/next-v1.md`](docs/performance/next-v1.md) for
the 5,000-Issue reference benchmark.

## Inspect a structural plan

`grit plan` exposes the exact `next/v1` decision together with immediate
parallel capacity and counterfactual Dependency layers:

```bash
grit plan --repo OWNER/REPO
grit plan --repo OWNER/REPO --assignee LOGIN --horizon 3 --json
```

`plan` and `next` share the same Working graph, ordered Pending mutations,
Execution scope, search, and cache. Pending priorities affect both the selected
rollout and structural Issue annotations. The plan decision preserves the same
input hash, completeness fields, and operation provenance as `next`; dependency
layers describe topology and do not predict scheduling or duration.

`parallel_now` is the complete Executable frontier for the active Execution
scope. `dependency_layers` covers every open Issue in the Repository: layer 0
contains all current Ready Issues, and each later finite layer follows the
latest layer of all its open blockers. Every Issue records assignment,
Execution-scope eligibility, and whether it is Executable now.

Cycles, opaque External blockers, unknown internal blockers, and their
affected descendants remain in `unresolved`; Grit does not assign them a
misleading finite layer. These layers describe dependency topology under
unlimited structural capacity. They are not dates, worker rounds, an ETA, or
a Critical Path. `plan/v1` therefore rejects `--workers` explicitly.

## Product decisions

The canonical domain language and accepted decisions live in
[`CONTEXT.md`](CONTEXT.md) and [`docs/adr`](docs/adr). The normative ranking
contract is [`docs/design/ranking-next-v1.md`](docs/design/ranking-next-v1.md).
