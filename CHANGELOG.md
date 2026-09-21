# Changelog

## 0.1.0

First release of Grit, a CLI for choosing executable work from GitHub Issues
and understanding what it can unblock.

- Synchronize native Issue dependencies into a validated local replica.
  Analyze the latest successful snapshot when GitHub is unavailable.
- List ready work, recommend a next Issue with bounded rollout evidence,
  inspect dependency layers, and diagnose cycles and priority conflicts.
- Create Draft Issues and queue edits, comments, labels, parent relationships,
  and Dependencies offline. Project pending work into analysis and reconcile
  it explicitly with GitHub.
- Generate a self-contained graph explorer with search, an accessible Issue
  table, dependency navigation, and precomputed recommendation evidence.
  Visible outcome counts and a completed-Issue list distinguish completed
  work from Issues closed as not planned or without a known reason.
- Generate a separate allowlisted public graph for public repositories,
  with a sealed publication boundary and a GitHub Pages workflow.
- Provide versioned JSON outputs for scripts and agents.
- Distribute native Linux and macOS archives for x86_64 and ARM64, with
  checksums, build metadata, documentation, and an MIT license. Every release
  target passes tests and an extracted-binary workflow before publication.

See the [installation guide](docs/installation.md) for supported system
versions and the [usage guide](docs/usage.md) for command contracts.
