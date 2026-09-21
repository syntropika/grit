# Changelog

## 0.1.1

- Use Grit's official OAuth App by default for browser login on GitHub.com:
  run `grit auth login` without supplying a Client ID.
- Keep custom app configuration through `--client-id` and
  `GRIT_GITHUB_CLIENT_ID`. GitHub Enterprise hosts require their own app.
- Update the installation guide and quickstart for the configured login.

## 0.1.0

First release of Grit, a CLI for choosing executable work from GitHub Issues
and understanding what it can unblock.

- Sign in using browser device authorization or a token
  supplied on standard input. Store credentials in the operating system's
  secure store; inspect the active account and remove Grit's saved login.
- Synchronize native Issue dependencies into a validated local replica.
  Analyze the latest successful snapshot when GitHub is unavailable.
- List ready work, recommend a next Issue with bounded rollout evidence,
  inspect dependency layers, and diagnose cycles and priority conflicts.
- Create Draft Issues and queue edits, comments, labels, parent relationships,
  and Dependencies offline. Project pending work into analysis and reconcile
  it explicitly with GitHub.
- Generate a self-contained graph explorer with a spatial network view,
  dependency layers, pan/zoom/fit controls, search, an accessible Issue table,
  and precomputed recommendation evidence.
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
