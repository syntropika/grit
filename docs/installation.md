# Install Grit

Grit is a standalone command-line executable. Its graph explorer assets are
embedded, so the released binary does not need a separate web server or Node.js.

## Download a binary

Choose the archive for your operating system and CPU from
[GitHub Releases](https://github.com/syntropika/grit/releases/latest).

| Platform | v0.1.0 archive | System requirement |
| --- | --- | --- |
| Linux x86_64 | [x86_64-unknown-linux-gnu](https://github.com/syntropika/grit/releases/download/v0.1.0/grit-v0.1.0-x86_64-unknown-linux-gnu.tar.gz) | glibc 2.35 or newer |
| Linux ARM64 | [aarch64-unknown-linux-gnu](https://github.com/syntropika/grit/releases/download/v0.1.0/grit-v0.1.0-aarch64-unknown-linux-gnu.tar.gz) | glibc 2.35 or newer |
| macOS Apple silicon | [aarch64-apple-darwin](https://github.com/syntropika/grit/releases/download/v0.1.0/grit-v0.1.0-aarch64-apple-darwin.tar.gz) | macOS 13 or newer |
| macOS Intel | [x86_64-apple-darwin](https://github.com/syntropika/grit/releases/download/v0.1.0/grit-v0.1.0-x86_64-apple-darwin.tar.gz) | macOS 13 or newer |

Use `uname -m` to check your CPU. On Linux, `aarch64` means ARM64; on macOS,
`arm64` means Apple silicon. Windows binaries are not provided in this release.

Download `SHA256SUMS` alongside the selected archive. On Linux, verify the
download with `sha256sum`; on macOS use `shasum -a 256`:

```bash
# Linux example; run from the directory containing both downloaded files.
grep 'grit-v0.1.0-x86_64-unknown-linux-gnu.tar.gz$' SHA256SUMS | sha256sum --check -
tar -xzf grit-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 grit-v0.1.0-x86_64-unknown-linux-gnu/grit "$HOME/.local/bin/grit"
"$HOME/.local/bin/grit" --version
```

Substitute your archive's target name on other platforms. Add
`$HOME/.local/bin` to your shell's `PATH` if it is not already present. The
archive also contains the license, README, and build metadata.

The macOS archives are not notarized. If your system's application policy
requires notarization, use a local source build instead.

## Install from source

With Rust 1.94.0 installed:

```bash
cargo +1.94.0 install --locked --git https://github.com/syntropika/grit --tag v0.1.0
grit --version
```

Cargo installs into its binary directory, usually `$HOME/.cargo/bin`.
This installs the tagged Grit repository; no crates.io package is required.

## Connect to GitHub

Authenticate using the [GitHub CLI](https://cli.github.com/):

```bash
gh auth login
grit sync --repo OWNER/REPO
grit next --repo OWNER/REPO
```

Grit first tries the active `gh` session, then a non-empty `GH_TOKEN`.
`gh` is optional when a token is already supplied through the environment.
The account must be able to read the repository; mutation commands also need
permission to make the requested changes.

The first successful synchronization creates the local snapshot used for
offline analysis. See the [usage guide](usage.md) for assignment scope,
priorities, offline edits, and graph generation.
