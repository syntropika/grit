# Grit

Grit treats the Issues and native Dependencies in one GitHub Repository as a
graph. GitHub remains the source of truth; Grit keeps a disposable Local
replica so later analysis can be fast and work offline.

The current executable tracer implements full synchronization and Executable
frontier enumeration from
[Issue #2](https://github.com/syntropika/grit/issues/2) and
[Issue #3](https://github.com/syntropika/grit/issues/3), plus the static graph
artifact, browser explorer, and fail-closed public projection from
[Issue #8](https://github.com/syntropika/grit/issues/8),
[Issue #12](https://github.com/syntropika/grit/issues/12), and
[Issue #13](https://github.com/syntropika/grit/issues/13).

## Build and test

Grit requires a stable Rust toolchain with Edition 2024 support.

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

The browser acceptance test is intentionally gated because the ordinary Rust
suite has no browser dependency. Run it explicitly with a Chrome-compatible
binary (Google Chrome by default):

```bash
GRIT_BROWSER=google-chrome cargo test --test graph_cli \
  generated_site_is_a_keyboard_accessible_offline_graph_explorer -- --ignored --exact
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

## Enumerate Executable work

`grit ready` refreshes the Local replica and lists the complete Executable
frontier in ascending Issue-number order:

```bash
grit ready --repo OWNER/REPO
grit ready --repo OWNER/REPO --assignee LOGIN --json
```

Without `--assignee`, the Execution scope contains Ready unassigned Issues.
With it, the scope contains Ready Issues assigned to that login. Assignment is
reported separately from Dependency readiness.

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
```

The target contains `index.html`, `app.css`, `app.js`, `graph.json`, and
`graph.schema.json`. The versioned JSON uses explicit `blocked` and `blocker`
edge roles, normalized Issue fields, precomputed layered positions,
operational counts, hashes, and provenance. Bodies, comments, raw API records,
and Operation markers are not part of the artifact.

The browser explorer searches by Issue number or title, renders the precomputed
dependency layers, and keeps graph selection synchronized with an accessible
Issue table. Its detail panel shows readiness, blockers, dependents, and the
canonical GitHub link. Labels appear only on hover, focus, or selection, and
the zoom controls never recalculate layout or ranking. All assets are local;
the browser does not contact GitHub or any other network service.

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
