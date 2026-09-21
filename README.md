# Grit

Grit treats the Issues and native Dependencies in one GitHub Repository as a
graph. GitHub remains the source of truth; Grit keeps a disposable Local
replica so later analysis can be fast and work offline.

The current executable tracer implements full synchronization and Executable
frontier enumeration from
[Issue #2](https://github.com/syntropika/grit/issues/2) and
[Issue #3](https://github.com/syntropika/grit/issues/3), plus canonical
Declared priority support from
[Issue #6](https://github.com/syntropika/grit/issues/6), and step-by-step
recommendations from
[Issue #11](https://github.com/syntropika/grit/issues/11) and
[Issue #14](https://github.com/syntropika/grit/issues/14), and structural plans
from [Issue #23](https://github.com/syntropika/grit/issues/23). It also generates
and explores the static graph from
[Issue #8](https://github.com/syntropika/grit/issues/8) and
[Issue #12](https://github.com/syntropika/grit/issues/12), including the
relationship filters and isolation tools from
[Issue #16](https://github.com/syntropika/grit/issues/16) and the dense-graph
guardrail from [Issue #17](https://github.com/syntropika/grit/issues/17).
It also generates a fail-closed public projection from
[Issue #13](https://github.com/syntropika/grit/issues/13).

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
GRIT_BROWSER=google-chrome cargo test --test public_graph_cli \
  sealed_public_bundle_executes_no_user_markup_or_external_request -- --ignored --exact
GRIT_BROWSER=google-chrome cargo test --test public_graph_cli \
  browser_network_audit_detects_an_external_request_attempt -- --ignored --exact
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

If GitHub cannot be reached, `ready` uses the latest valid Local replica and
reports its unchanged `synced_at`. A replica is accepted only when its schema,
Repository scope, timestamp, and deterministic content hash validate. If no
valid replica exists, the command fails instead of inventing an empty graph.

## Generate a static graph artifact

`grit graph` writes a complete static site without a live service or
browser-side GitHub client:

```bash
grit graph --repo OWNER/REPO --output site/
grit graph --repo OWNER/REPO --output site/ --json
grit graph --repo OWNER/REPO --output site/ --assignee LOGIN --horizon 3
```

The target contains `index.html`, `app.css`, `graph-query.js`, `network-view.js`, `app.js`,
`graph.json`, and `graph.schema.json`. The versioned JSON uses explicit
`blocked` and `blocker`
edge roles, normalized Issue fields, precomputed layered positions,
operational counts, hashes, and provenance. Bodies, comments, raw API records,
and Operation markers are not part of the artifact.

The `grit.graph-artifact/v2` artifact also carries the exact precomputed `next/v1` analysis and the
matching structural `plan` for its Execution scope and horizon. Its summary
shows the recommendation, decisive reason, distinct runner-up, search
completeness, immediate parallel work, unresolved cycles, and unknown External
blockers. Selecting the recommendation highlights its rollout, relevant
blockers, and unlocked outcomes. Node size can display ranked Unlock behavior
or PageRank buckets; node color can display readiness, state, or Declared
priority. The browser only presents values produced by the binary and never
recalculates ranking or PageRank.

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

Before publication, Grit reparses and validates every JSON document in the
complete public staging directory, verifies its closed manifest and local
runtime policy, scans every byte for excluded fixture values and common secret
signatures, and only then atomically exchanges it with the previous sealed
bundle. A failed gate leaves the previous bundle intact. Source maps, logs,
remote assets, analytics, API clients, and service workers are rejected.

### Deploy the sealed graph to GitHub Pages

The bundled Pages workflow builds Grit and runs the same fail-closed public
generation path on pushes to `main`, every six hours, and on explicit manual
runs. Build and deployment use separate jobs. Only the build job can read
Issues; its `GITHUB_TOKEN` is exposed to Grit as `GH_TOKEN` only for graph
generation. The deployment job receives a separate job token that can write
Pages but cannot read Issues.

The workflow uploads exactly `site-public/`; it never uploads the Repository,
Local replica, outbox, or build workspace. All Actions are pinned to immutable
commit SHAs, and one cancelable concurrency group prevents an older run from
replacing a newer bundle. The Repository must be publicly visible: generation
fails before publication when GitHub reports private, internal, unknown, or
unreachable visibility.

## Recommend the next Issue

`grit next` evaluates rollouts of up to three Executable completions. Three is
the default Planning horizon; horizons one and two remain available:

```bash
grit next --repo OWNER/REPO
grit next --repo OWNER/REPO --horizon 1
grit next --repo OWNER/REPO --assignee LOGIN --horizon 3 --json
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
For longer horizons, Grit exhaustively explores successors until the
deterministic 8,192-state budget is reached. Exhausted searches report
`truncated_by: ["state_budget"]`, set `search_complete` and
`global_optimum_claimed` to false, and scope the runner-up to `explored`.
Multi-step critical-route discovery lands separately; until then, a blocked P0
at horizons two or three similarly reports `p0_frontier` instead of making an
unsupported optimum claim.

Robot output reports both snapshot and effective-input hashes, metric states,
the global runner-up comparison with its canonical presentation message,
mode-specific supporting reasons, and whether the result is a close structural
tie. If no Issue is Executable, the command succeeds with a null recommendation
and categorized blocker counts. Like `ready`, it attempts a pull Synchronization
and falls back to the latest valid Local replica without mutating GitHub.

## Inspect a structural plan

`grit plan` exposes the exact `next/v1` decision together with immediate
parallel capacity and counterfactual Dependency layers:

```bash
grit plan --repo OWNER/REPO
grit plan --repo OWNER/REPO --assignee LOGIN --horizon 3 --json
```

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
