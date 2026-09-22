# Changelog

## 0.2.1

- Name the source package `hyfa`, matching the executable and agent skill.
- Add crates.io distribution so installation uses `cargo install hyfa --locked`.
  GitHub binary archives remain available for Linux and macOS.

## 0.2.0

- Rename the CLI to `hyfa` and the repository to [`syntropika/hyfa`](https://github.com/syntropika/hyfa).
- Ship `hyfa-v0.2.0-TARGET.tar.gz` archives containing the `hyfa` executable.
  Update the graph explorers, bundled agent skill, documentation, and workflows.
- Use `HYFA_*` environment variables, `hyfa.*` JSON schema identifiers, a
  `hyfa` local state directory, and a separate Hyfa credential-store entry.
  Scripts and integrations must adopt these names before upgrading.
- Finish reconciling pending changes with the previous CLI before upgrading,
  then run `hyfa auth login` and `hyfa sync --repo OWNER/REPO`. Existing state
  and saved credentials are preserved but are not migrated automatically.
- Continue recognizing historical operation markers in GitHub content so they
  remain hidden and ambiguous writes can still be identified.

## 0.1.1

- Use the official OAuth App by default for browser login on GitHub.com,
  without requiring users to supply a Client ID.
- Keep custom app configuration through `--client-id` and the environment.
  GitHub Enterprise hosts require their own app.
- Update the installation guide and quickstart for the configured login.
- Bundle a usage skill and install it offline with the `skill install`
  subcommand, using the native `skillinstaller` library for provider destinations.
- Prepare source packaging with an organization-prefixed package name.
  Include the agent skill in source and binary distributions; registry
  publication is deferred.

## 0.1.0

First public release of the CLI, for choosing executable work from GitHub Issues
and understanding what it can unblock.

- Sign in using browser device authorization or a token
  supplied on standard input. Store credentials in the operating system's
  secure store; inspect the active account and remove the saved login.
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
