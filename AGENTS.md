# Working on Grit

Read [CONTEXT.md](CONTEXT.md) and the relevant [ADRs](docs/adr/) before changing
domain behavior. [CONTRIBUTING.md](CONTRIBUTING.md) provides build and validation
commands. [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md) describes
the GitHub issue workflow. Use the originating Issue number when one exists;
do not invent an Issue or an approval requirement for routine work.

## Repository map

- `src/cli.rs`: argument parsing, command orchestration, and output envelopes.
- `src/auth.rs`, `src/auth/`: authentication, device authorization, and
  host-scoped secure credential storage.
- `src/github.rs`, `src/synchronization.rs`, `src/dependency_events.rs`: GitHub
  access, incremental continuity, and full-refresh fallback.
- `src/model.rs`, `src/store.rs`, `src/atomic_file.rs`: normalized data and
  atomic persistence.
- `src/outbox.rs`, `src/working_graph.rs`, and mutation modules: durable intent,
  provisional projection, conflict handling, and reconciliation.
- `src/operational/`, `src/ranking/`, `src/plan.rs`, `src/triage/`: executable
  work, bounded ranking, structural layers, and diagnostics.
- `src/graph/`: typed graph artifacts, layout, the embedded explorer, and the
  separate sealed public export.
- `tests/`: CLI integration fixtures, browser harnesses, and JavaScript checks.

Check the actual module layout before adding files. Prefer extending the
responsible module over expanding unrelated CLI orchestration.

## Invariants to preserve

1. GitHub is authoritative. Publish a Local replica only after a complete,
   validated synchronization. Failed refreshes preserve the previous replica
   and `synced_at`.
2. Pending mutations belong in the outbox. Analysis projects them into the
   Working graph but never replays remote writes. Reconciliation is explicit.
3. Recommendations contain only executable open Issues in the chosen scope.
   Closed Issues can satisfy dependencies and appear in history; they never
   become operational candidates. Keep assignment separate from readiness.
4. Preserve deterministic ordering, declared search limits, cache identity,
   and ordered pending provenance. Drafts use stable identities; internal
   synthetic numbers must not appear as GitHub Issue numbers.
5. `next`, `plan`, and the full graph explorer share the same effective input
   and precomputed decision. The browser presents analysis; it does not rerun
   ranking or layout.
6. Public export is a separate allowlisted contract with a live visibility
   check and a fail-closed sealing boundary. Do not publish the full private
   artifact as a shortcut. Bodies, comments, pending intent, credentials, raw
   identities, and external blocker identities remain excluded.
7. Versioned CLI and artifact outputs are integration contracts. Review schema
   compatibility and update meaningful consumer tests when fields change.

## Working and verification

- Preserve existing edits and coordinate ownership when sharing a worktree.
- Use a separate Cargo target directory per worktree. Never package a binary
  merely because its version string matches; build it from the release commit.
- Use mocked GitHub and temporary state in tests. Never point tests at a real
  repository or a user's existing replica/outbox.
- Set `GRIT_NO_KEYRING=1` in CLI fixtures so tests cannot discover an operator's
  saved login. Auth unit tests inject their own HTTP and credential-store fakes.
- Run the relevant checks from `CONTRIBUTING.md`. Changes to generated HTML
  require real browser verification, including keyboard and mobile behavior.
  Public renderer changes also require the sealing and browser-egress tests.
- Keep the README focused on user value and getting started. Put command
  detail in `docs/usage.md`, contributor setup in `CONTRIBUTING.md`, and agent
  instructions here. Persist all prose and generated UI copy in English.
- Store temporary reports, screenshots, and traces outside the checkout under
  `/home/cerberus/.codex-artifacts/home/cerberus/Projects/grit/` in this workspace.
  Do not commit credentials, Local replicas, outboxes, or generated exports.
