# Contributing to Grit

Grit is a Rust CLI with an embedded static graph explorer. Start with
[CONTEXT.md](CONTEXT.md) for the domain model and the relevant
[architecture decisions](docs/adr/). The [ranking contract](docs/design/ranking-next-v1.md)
defines `next/v1`; change the implementation and its documented contract together.

For using the released CLI, see the [usage guide](docs/usage.md).
[AGENTS.md](AGENTS.md) contains instructions for coding agents working in this repository.

## Development setup

Use Rust 1.94.0, Cargo, and Node.js 22 or newer. Linux and macOS are the release
platforms. Some filesystem operations and test fixtures are Unix-specific;
Windows is not currently a supported build target.

```bash
git clone https://github.com/syntropika/grit.git
cd grit
rustup toolchain install 1.94.0 --profile minimal --component rustfmt --component clippy
cargo +1.94.0 build --locked
cargo +1.94.0 run -- --help
```

The normal test suite uses local GitHub fixtures and needs no GitHub credentials.
For manual experiments, use a repository you control and a separate
`GRIT_STATE_DIR`. Analysis commands may pull remote data; mutation commands
can change GitHub when credentials and connectivity are available.

## Checks

Run checks relevant to your change. Before merging code, run:

```bash
cargo +1.94.0 fmt --all -- --check
cargo +1.94.0 clippy --locked --all-targets --all-features -- -D warnings
cargo +1.94.0 test --locked --all-targets --all-features
node --test tests/*.test.js
git diff --check
```

Keep separate `CARGO_TARGET_DIR` directories for different worktrees. Sharing
one target directory can replace the CLI executable while another worktree's
integration tests are using it.

### Browser behavior

Browser acceptance tests are ignored by the normal Rust suite. Run them when
changing graph rendering, filters, generated artifacts, or public export:

```bash
GRIT_BROWSER=google-chrome cargo +1.94.0 test --locked --test graph_cli -- --ignored
GRIT_BROWSER=google-chrome cargo +1.94.0 test --locked --test graph_working_browser_cli -- --ignored
GRIT_BROWSER=google-chrome cargo +1.94.0 test --locked --test public_graph_cli -- --ignored
```

Set `GRIT_BROWSER` to a Chrome-compatible executable. Tests launch disposable
profiles and check offline behavior, keyboard interactions, and public egress.

### Performance-sensitive changes

Use optimized builds for the existing ranking and graph browser gates:

```bash
cargo +1.94.0 test --locked --release \
  ranking::performance_tests::repository_scale_profile_meets_the_next_v1_latency_budget \
  -- --ignored --exact --nocapture
GRIT_BROWSER=google-chrome cargo +1.94.0 test --locked --release \
  graph::benchmark::dense_graph_browser_benchmark -- --ignored --exact --nocapture
```

Run these without concurrent builds or benchmarks and retain the phase timings
when investigating failures. See [ranking measurements](docs/performance/next-v1.md)
and [browser measurements](docs/benchmarks/graph-browser.md) for the fixtures and limits.

## Where documentation belongs

| File | Audience and purpose |
| --- | --- |
| `README.md` | Users: purpose, installation, and first useful workflow |
| `docs/installation.md` | Users: supported binaries, checksums, and source installation |
| `docs/usage.md` | Users and integrations: command behavior, options, and output contracts |
| `CONTRIBUTING.md` | Contributors: development setup, validation, and release process |
| `AGENTS.md` | Coding agents: repository map, invariants, and working instructions |
| `CONTEXT.md` and `docs/adr/` | Shared domain language and design decisions |

Write persisted prose in English. Keep temporary screenshots, traces, benchmark
logs, and review reports outside the repository; commit only intentional
documentation and fixtures.

## Releases

The release workflow builds native Linux and macOS binaries for x86_64 and
ARM64 using Rust 1.94.0. Pull requests validate the same packaging path without
publishing a release. A version tag matching `Cargo.toml` triggers publication
after every target passes its tests and archive smoke checks.

For a release, update the package version and lockfile, summarize user-visible
changes in `CHANGELOG.md`, and merge the validated changes. Create the matching
`vVERSION` tag from that merged commit. The workflow produces platform archives
and `SHA256SUMS`, uploads the complete set to a draft release, and publishes it
only after verification. Published versions are not overwritten; corrections
use a new version. Confirm the release's downloaded binary reports the intended
version and includes the current commands.

GitHub Pages has a separate workflow for the reduced public graph. It is not
the binary release or the full local explorer.
