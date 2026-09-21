# Grit

Grit treats the Issues and native Dependencies in one GitHub Repository as a
graph. GitHub remains the source of truth; Grit keeps a disposable Local
replica so later analysis can be fast and work offline.

The current executable tracer implements full and incremental synchronization
plus Executable frontier enumeration from
[Issue #2](https://github.com/syntropika/grit/issues/2) and
[Issue #3](https://github.com/syntropika/grit/issues/3), with ordinary Issue
changes from [Issue #4](https://github.com/syntropika/grit/issues/4),
Dependency-event continuity from
[Issue #5](https://github.com/syntropika/grit/issues/5), and canonical
Declared priority support from
[Issue #6](https://github.com/syntropika/grit/issues/6).

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

## Product decisions

The canonical domain language and accepted decisions live in
[`CONTEXT.md`](CONTEXT.md) and [`docs/adr`](docs/adr). The normative ranking
contract is [`docs/design/ranking-next-v1.md`](docs/design/ranking-next-v1.md).
