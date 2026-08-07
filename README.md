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
updates from [Issue #9](https://github.com/syntropika/grit/issues/9), and exact
horizon-one recommendations from
[Issue #11](https://github.com/syntropika/grit/issues/11), and offline Priority
intent projection from [Issue #15](https://github.com/syntropika/grit/issues/15).
It also reconciles ordered Pending mutations from
[Issue #20](https://github.com/syntropika/grit/issues/20) and projects offline
native-Dependency changes from
[Issue #24](https://github.com/syntropika/grit/issues/24), and creates safely
recoverable Draft Issues from
[Issue #26](https://github.com/syntropika/grit/issues/26).

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

Pending Priority and native-Dependency mutations are applied in order over the
Local replica to form the Working graph used by `ready` and `next`. Their JSON
identifies every affected Issue and result with `pending: true` and the
responsible operation IDs. Analysis may pull newer GitHub state, but it never
replays or writes the outbox; reconciliation is a separate explicit operation.

If GitHub cannot be reached, `ready` uses the latest valid Local replica and
reports its unchanged `synced_at`. A replica is accepted only when its schema,
Repository scope, timestamp, and deterministic content hash validate. If no
valid replica exists, the command fails instead of inventing an empty graph.

## Recommend the next Issue

`grit next` evaluates every Issue in the active Executable frontier exactly at
horizon one:

```bash
grit next --repo OWNER/REPO --horizon 1
grit next --repo OWNER/REPO --assignee LOGIN --horizon 1 --json
```

The `next/v1` policy first enforces Executable P0 and one-step P0-route gates.
It then compares real AND-aware Unlock sets, downstream Priority composition,
the first step's Declared priority, a quantized PageRank tie-break, and finally
the Stable node key. It never recommends blocked or out-of-scope work.

Robot output reports both snapshot and effective-input hashes, metric states,
the global runner-up comparison, structured reasons, and whether the result is
a close structural tie. Pending recommendations, Issue references, comparison
evidence, and reasons carry operation provenance. If no Issue is Executable,
the command succeeds with a null recommendation and categorized blocker
counts. Like `ready`, it attempts a pull Synchronization and falls back to the
latest valid Local replica without mutating GitHub.

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

## Product decisions

The canonical domain language and accepted decisions live in
[`CONTEXT.md`](CONTEXT.md) and [`docs/adr`](docs/adr). The normative ranking
contract is [`docs/design/ranking-next-v1.md`](docs/design/ranking-next-v1.md).
