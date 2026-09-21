# Grit

Grit treats the Issues and native Dependencies in one GitHub Repository as a
graph. GitHub remains the source of truth; Grit keeps a disposable Local
replica so later analysis can be fast and work offline.

Grit supports full and incremental synchronization, Dependency-event continuity,
Executable frontier enumeration, canonical Declared priority initialization,
native Dependency mutations, Declared priority updates, and deterministic
static graph artifacts, with actionable triage diagnostics and bounded multistep
next-work recommendations with an exact horizon-one option. Its offline browser explorer presents the precomputed
Issue graph with synchronized network and accessible table selection,
composable filters, Dependency relationship isolation, and a bounded view for dense graphs.
Public exports use a separate allowlisted model after a live Repository visibility check.
Unavailable Priority and Dependency updates are queued durably and projected into analysis with explicit Pending provenance.
Grit also creates recoverable Draft Issues with stable temporary identities and marker-based reconciliation.
Title, body, state, and assignment edits use field-aware Pending mutations.
Structural plans share the cached Working-graph analysis used by next-work recommendations.

## Build and test

Grit requires a stable Rust toolchain with Edition 2024 support.

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
node --test tests/network_view.test.js
```

The browser acceptance test is intentionally gated because the ordinary Rust
suite has no browser dependency. Run it explicitly with a Chrome-compatible
binary (Google Chrome by default):

```bash
GRIT_BROWSER=google-chrome cargo test --test graph_cli \
  generated_site_is_a_keyboard_accessible_offline_graph_explorer -- --ignored --exact

GRIT_BROWSER=google-chrome cargo test --release \
  graph::benchmark::dense_graph_browser_benchmark -- --ignored --exact --nocapture
```

The test launches an ephemeral headless profile with background networking
disabled and host resolution denied.

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

The first synchronization retrieves every page of Issues and Repository Issue
comments, then the native `blocked_by` Dependencies for each Issue. Pull
Requests returned by the Issues endpoint are excluded. Later runs request
ordinary Issue and comment changes from an overlapped `updated_at` watermark,
paginate them in stable creation order, upsert them by stable GitHub identity,
and refresh Dependencies only for Issues whose ordinary fields changed. A
scoped ETag is reused only when the preceding delta proved that the complete
representation fit in fewer than 100 items; it is never treated as a
Repository-wide continuity guarantee.

Dependency additions and removals use a separate Repository event checkpoint.
Grit canonicalizes the mirrored `blocked_by` and `blocking` events into one
edge direction and fetches any referenced in-scope Issue absent from the Local
replica. A one-request GraphQL count probe also detects Issues that disappeared
without a REST delta tombstone. If that count diverges, the checkpoint
disappears from the available event history, no event-ID anchor exists, an
event cannot be interpreted safely, or an Issue was transferred or deleted,
Grit discards the partial delta and performs a Full reconciliation before
publishing anything.

Grit writes only a normalized model and atomically replaces the previous valid
replica after every required page has completed. A failed or rate-limited delta
leaves the prior replica, internal watermark, and visible `synced_at` intact.

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

## Create Draft Issues

Create a Draft Issue locally when authoring must continue without GitHub:

```bash
grit create --repo OWNER/REPO --title "Prepare the migration" --body "Acceptance notes"
grit create --repo OWNER/REPO --title "Prepare the migration" --json
```

Creation returns a stable Temporary Issue ID and a key such as
`OWNER/REPO#draft:TEMPORARY_ID`. The Draft participates provisionally in
`ready` and `next`; that key can also be passed to `block` or `unblock` before
GitHub assigns an Issue number. `grit reconcile` creates referenced Drafts
before their dependent operations, stores the permanent number and node ID,
and retains the temporary alias.

Every non-idempotent create has a random, non-secret Operation marker persisted
before the request. Grit embeds it in an invisible Markdown comment, removes it
from normalized and user-facing data, and uses it only to recover an ambiguous
response. Exactly one remote match is accepted; zero or multiple matches stay
unresolved and are never retried blindly.

## Edit Issue fields

`update` changes exactly one logical field per invocation:

```bash
grit update OWNER/REPO#42 --title "A clearer title"
grit update OWNER/REPO#42 --body "Revised Markdown"
grit update OWNER/REPO#42 --state closed
grit update OWNER/REPO#42 --assignee alice --assignee bob
grit update OWNER/REPO#42 --clear-assignees
grit update OWNER/REPO#42 --priority p1
```

Title, body, state, and assignment also accept a Draft key such as
`OWNER/REPO#draft:TEMPORARY_ID`. Canonical Issues are updated online when
GitHub is available; otherwise Grit records the base and desired values in the
outbox and projects the desired field into `ready` and `next` without changing
the Local replica. `grit reconcile` applies a Pending field only when GitHub
still matches its base, treats the desired remote value as already satisfied,
and exposes incompatible base/local/remote values as a conflict. Resolve a
conflict explicitly with `grit resolve OPERATION --repo OWNER/REPO --local` or
`--remote`; resolution always performs a fresh GitHub read before any write.

## Triage graph problems

`grit triage` explains actionable graph problems without treating blocked work
as executable:

```bash
grit triage --repo OWNER/REPO
grit triage --repo OWNER/REPO --assignee LOGIN --json
```

Diagnostics cover blocked P0 Issues, open or unknown External blockers, cyclic
SCCs, assigned Ready work, and Priority conflicts. Each subject reports
Dependency readiness, availability, and membership in the active Execution
scope separately. Closed historical topology produces no diagnostic. Output is
deterministic and uses stable reason codes and Issue references.

Like other analysis commands, `triage` attempts only pull Synchronization. If
GitHub is unavailable, it uses the latest valid Local replica and reports the
unchanged `synced_at`.

## Change native Dependencies

Use full Issue references so the direction remains explicit:

```bash
grit block OWNER/REPO#42 --by OWNER/REPO#7
grit unblock OWNER/REPO#42 --by OWNER/REPO#7 --json
```

The first command means “Issue #42 is blocked by Issue #7.” Grit writes the
native GitHub `blocked_by` relationship, then performs a complete synchronized
readback before atomically replacing the Local replica. Repeating either
operation uses set semantics: an existing edge can be added again and an absent
edge can be removed again without error. If GitHub is unavailable or the write
outcome is ambiguous and a valid Local replica exists, Grit queues the intent,
leaves that replica unchanged, and immediately projects the edge into offline
readiness and ranking. A blocker from another Repository is preserved as an
opaque External blocker with unknown state until GitHub can synchronize it.

Apply queued work explicitly after connectivity returns:

```bash
grit reconcile --repo OWNER/REPO
grit reconcile --repo OWNER/REPO --json
```

Reconciliation refreshes GitHub first, applies each independent mutation
branch, checkpoints attempted writes, and finishes with a complete synchronized
readback before publishing the Local replica. Dependency changes use idempotent
set semantics, so a retry after an ambiguous response observes the desired edge
instead of duplicating or conflicting with it. A failed operation blocks only
mutations that declare it as a prerequisite.


## Generate a static graph artifact

`grit graph` writes a complete static site without a live service or
browser-side GitHub client:

```bash
grit graph --repo OWNER/REPO --output site/
grit graph --repo OWNER/REPO --output site/ --json
```

The target contains `index.html`, `app.css`, `graph-query.js`, `network-view.js`, `app.js`,
`graph.json`, and `graph.schema.json`. The versioned JSON uses explicit
`blocked` and `blocker`
edge roles, normalized Issue fields, precomputed layered positions,
operational counts, hashes, and provenance. Bodies, comments, raw API records,
and Operation markers are not part of the artifact.

The browser explorer searches by Issue number or title, renders the precomputed
dependency layers, and keeps graph selection synchronized with an accessible
Issue table. Its detail panel shows readiness, blockers, dependents, and the
canonical GitHub link. Labels appear only on hover, focus, or selection, and
the zoom controls never recalculate layout or ranking. All assets are local;
the browser does not contact GitHub or any other network service.

Readiness, state, Declared priority, area-label, assignee, and disconnected
component filters compose over the static data. Area, multi-component, and
optional Project controls are omitted when their source data is absent; the
current Issues-only artifact has no Project membership and therefore renders
no Project filter. Root-and-depth isolation follows Dependencies in both
directions to bound the visible
neighborhood while preserving every precomputed position and metric. Separate
actions highlight transitive upstream blockers, transitive downstream
dependents, or the shortest directed path between two selected nodes. The
table mirrors the filtered nodes and relationship annotations, including all
matching results outside a constrained network window. `Clear view` restores
the initial graph view.

The measured full-network range is 5,000 nodes and 20,000 edges. Larger
artifacts open with an at-most-500-node overview seeded from Ready Issues.
Search and the complete accessible table remain available; selecting a result
opens its bounded neighborhood, while rendering the full network requires an
explicit action. Every view reuses positions produced by the binary. The
browser does not run layout or ranking. See the reproducible measurements and
environment in [`docs/benchmarks/graph-browser.md`](docs/benchmarks/graph-browser.md).

Generation validates the closed artifact model in a staging directory before
replacing the target. Regenerating from the same effective Local replica is
byte-stable. `synced_at` is the only time field and changes only when the input
replica itself has a different synchronization timestamp.

### Generate an allowlisted public graph

`--public` first obtains current Repository metadata from GitHub and refuses to
generate unless both `visibility: public` and `private: false` confirm the
requested Repository. A Local replica never substitutes for this live
visibility check.

```bash
grit graph --repo OWNER/REPO --output site-public/ --public
grit graph --repo OWNER/REPO --output site-public/ --public \
  --public-label-prefix area: --public-label-prefix priority:
grit graph --repo OWNER/REPO --output site-public/ --public \
  --public-include-assignees
```

PublicGraphV1 contains only open Issues from that Repository, canonical Issue
URLs, titles, state, readiness, internal Dependency edges, and positions
recomputed from the allowlisted model. Labels require an explicit category
prefix. Assignee logins require a separate explicit opt-in. Bodies, comments,
closed history, raw/internal IDs, Project data, Pending mutations, and Operation
markers are absent.

An unsatisfied External blocker is represented only by `external_unknown` on
the affected public Issue. Its owner, Repository, number, title, count, and
topology never enter the public model. Changes to excluded history, private
text, identities, assignees, labels, or External-blocker details cannot change
the public hash, readiness, ordering, internal edges, or coordinates.

## Product decisions

The canonical domain language and accepted decisions live in
[`CONTEXT.md`](CONTEXT.md) and [`docs/adr`](docs/adr). The normative ranking
contract is [`docs/design/ranking-next-v1.md`](docs/design/ranking-next-v1.md).
