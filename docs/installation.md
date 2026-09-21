# Install Grit

Grit is a standalone command-line executable. Its graph explorer assets are
embedded, so the released binary does not need a separate web server or Node.js.

## Download a binary

Choose the archive for your operating system and CPU from
[GitHub Releases](https://github.com/syntropika/grit/releases/latest).

| Platform | v0.1.1 archive | System requirement |
| --- | --- | --- |
| Linux x86_64 | [x86_64-unknown-linux-gnu](https://github.com/syntropika/grit/releases/download/v0.1.1/grit-v0.1.1-x86_64-unknown-linux-gnu.tar.gz) | glibc 2.35 or newer |
| Linux ARM64 | [aarch64-unknown-linux-gnu](https://github.com/syntropika/grit/releases/download/v0.1.1/grit-v0.1.1-aarch64-unknown-linux-gnu.tar.gz) | glibc 2.35 or newer |
| macOS Apple silicon | [aarch64-apple-darwin](https://github.com/syntropika/grit/releases/download/v0.1.1/grit-v0.1.1-aarch64-apple-darwin.tar.gz) | macOS 13 or newer |
| macOS Intel | [x86_64-apple-darwin](https://github.com/syntropika/grit/releases/download/v0.1.1/grit-v0.1.1-x86_64-apple-darwin.tar.gz) | macOS 13 or newer |

Use `uname -m` to check your CPU. On Linux, `aarch64` means ARM64; on macOS,
`arm64` means Apple silicon. Windows binaries are not provided in this release.

Download `SHA256SUMS` alongside the selected archive. On Linux, verify the
download with `sha256sum`; on macOS use `shasum -a 256`:

```bash
# Linux example; run from the directory containing both downloaded files.
grep 'grit-v0.1.1-x86_64-unknown-linux-gnu.tar.gz$' SHA256SUMS | sha256sum --check -
tar -xzf grit-v0.1.1-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 grit-v0.1.1-x86_64-unknown-linux-gnu/grit "$HOME/.local/bin/grit"
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
cargo +1.94.0 install --locked --git https://github.com/syntropika/grit --tag v0.1.1
grit --version
```

Cargo installs into its binary directory, usually `$HOME/.cargo/bin`.
This installs the tagged Grit repository; no crates.io package is required.

## Install the agent skill

Run this from a project to install Grit's bundled usage instructions for
agents that read `.agents/skills`:

```bash
grit skill install
```

The command creates `.agents/skills/grit/SKILL.md` in the current directory.
It runs offline, needs no GitHub login, and copies the skill embedded in the
binary. The installed skill covers recommendations, Issue creation,
assignment, comments, dependencies, pending changes, and graph generation.

Select specific providers or another project explicitly:

```bash
grit skill providers
grit skill install --providers codex,claude-code --project-root /path/to/project
```

Providers sharing `.agents/skills` use one physical copy. Claude Code uses
`.claude/skills/grit`. Existing installations are preserved unless you pass
`--force`, which replaces the existing Grit skill directory. Installation
prints the actual destinations.

User scope is available for providers with dedicated user directories, for
example `grit skill install --scope user --providers claude-code`. In this
release, user scope is rejected for shared-path providers such as Codex and
`universal` because the installer's user mapping does not match their discovery
paths. Use project scope for those providers.

## Connect to GitHub

Grit calls GitHub's API directly from Rust. Choose one of the following ways
to authenticate.

### Sign in with a browser

```bash
grit auth login
grit auth status
```

Open the verification address printed by Grit and enter the displayed code.
Grit validates the resulting token and saves it in the operating system's
secure credential store: macOS Keychain or a Linux Secret Service session.
It never falls back to a plaintext credential file.

On GitHub.com, Grit uses its official OAuth App by default. Each user
authorizes that app; users do not need to register their own. An organization
that restricts OAuth apps may require its owner's approval before organization
repositories can be accessed.

To use a custom app, pass its public ID with `--client-id` or set
`GRIT_GITHUB_CLIENT_ID`. The command-line flag takes precedence over the
environment variable, which takes precedence over the official GitHub.com
default. An invalid or empty override is rejected. No Client Secret is needed.

Grit reuses its saved login on later commands. The official app is configured
to issue tokens without a scheduled expiration. You may need to sign in again
if access is revoked or GitHub invalidates the token.

If a custom app issues an expiring token, Grit records its expiration and asks
you to log in again after it expires. This release does not refresh OAuth
tokens automatically.

### Save an existing token

Pipe a token from your password manager or another trusted secret source into
`grit auth login --with-token`. The command only accepts piped standard input,
so the token does not need to appear in a command argument or shell history.
This method needs no OAuth App registration and uses the same secure store.

### Use an environment token

Supply a GitHub token through `GH_TOKEN` in your shell or CI secret
configuration. This works without a desktop keychain.

Credential precedence is **`GH_TOKEN` → Grit's saved login**.
The account must be able to read the repository; mutation commands also need
permission to make the requested changes. Then run:

```bash
grit sync --repo OWNER/REPO
grit next --repo OWNER/REPO
```

Use `grit auth status --json` to inspect the active account and credential
source without exposing the token. `grit auth logout` removes only Grit's
saved credential; it does not revoke the token on GitHub or modify `GH_TOKEN`.

For GitHub Enterprise, pass `--hostname github.example.com` to auth commands
and set both `GRIT_GITHUB_HOST=github.example.com` and
`GRIT_GITHUB_API_URL=https://github.example.com/api/v3/` for repository commands.
Browser login on an Enterprise host requires a Client ID from an OAuth App
registered on that instance with Device Flow enabled; Grit's GitHub.com app
is not used on other hosts. See
[GitHub's registration guide](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app).
Credentials are stored separately for each host. Set `GRIT_NO_KEYRING=1`
to disable secure-store discovery in headless sessions or isolated tests;
`GH_TOKEN` remains available.

The first successful synchronization creates the local snapshot used for
offline analysis. See the [usage guide](usage.md) for assignment scope,
priorities, offline edits, and graph generation.
