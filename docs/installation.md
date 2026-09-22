# Install Hyfa

Hyfa is a standalone command-line executable. Its graph explorer assets are
embedded, so the released binary does not need a separate web server or Node.js.

## Download a binary

Choose the archive for your operating system and CPU from
[GitHub Releases](https://github.com/syntropika/hyfa/releases/latest).

| Platform | v0.2.1 archive | System requirement |
| --- | --- | --- |
| Linux x86_64 | [x86_64-unknown-linux-gnu](https://github.com/syntropika/hyfa/releases/download/v0.2.1/hyfa-v0.2.1-x86_64-unknown-linux-gnu.tar.gz) | glibc 2.35 or newer |
| Linux ARM64 | [aarch64-unknown-linux-gnu](https://github.com/syntropika/hyfa/releases/download/v0.2.1/hyfa-v0.2.1-aarch64-unknown-linux-gnu.tar.gz) | glibc 2.35 or newer |
| macOS Apple silicon | [aarch64-apple-darwin](https://github.com/syntropika/hyfa/releases/download/v0.2.1/hyfa-v0.2.1-aarch64-apple-darwin.tar.gz) | macOS 13 or newer |
| macOS Intel | [x86_64-apple-darwin](https://github.com/syntropika/hyfa/releases/download/v0.2.1/hyfa-v0.2.1-x86_64-apple-darwin.tar.gz) | macOS 13 or newer |

Use `uname -m` to check your CPU. On Linux, `aarch64` means ARM64; on macOS,
`arm64` means Apple silicon. Windows binaries are not provided in this release.

Download `SHA256SUMS` alongside the selected archive. On Linux, verify the
download with `sha256sum`; on macOS use `shasum -a 256`:

```bash
# Linux example; run from the directory containing both downloaded files.
grep 'hyfa-v0.2.1-x86_64-unknown-linux-gnu.tar.gz$' SHA256SUMS | sha256sum --check -
tar -xzf hyfa-v0.2.1-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 hyfa-v0.2.1-x86_64-unknown-linux-gnu/hyfa "$HOME/.local/bin/hyfa"
"$HOME/.local/bin/hyfa" --version
```

Substitute your archive's target name on other platforms. Add
`$HOME/.local/bin` to your shell's `PATH` if it is not already present. The
archive also contains the license, README, and build metadata.

The macOS archives are not notarized. If your system's application policy
requires notarization, use a local source build instead.

## Install from crates.io

With Rust 1.94.0 or newer installed:

```bash
cargo install hyfa --locked --version 0.2.1
hyfa --version
```

The package and executable are both named `hyfa`. Cargo installs into its
binary directory, usually `$HOME/.cargo/bin`.

## Install from the GitHub source

With Rust 1.94.0 installed:

```bash
cargo +1.94.0 install --locked --git https://github.com/syntropika/hyfa --tag v0.2.1
hyfa --version
```

Cargo installs into its binary directory, usually `$HOME/.cargo/bin`.
This installs the tagged Hyfa repository; no crates.io package is required.

## Upgrade from versions before 0.2.0

Version 0.2.0 introduces the Hyfa name across the executable, environment
variables, JSON schema identifiers, agent skill, and local storage.
Before upgrading, use the previous executable to reconcile pending changes
for every repository. Keep that installation and its state until every
pending operation is resolved; copying old state files into the new directory
is not supported.

Install Hyfa, replace environment settings with their `HYFA_*` equivalents,
and update integrations to expect `hyfa.*` schema identifiers. Run
`hyfa auth login` and `hyfa sync --repo OWNER/REPO` for each repository to
establish the new credentials and local replicas. `GH_TOKEN` still works.
Previous state and credentials are left intact; they are not imported or
deleted. Reinstall the bundled agent skill with `hyfa skill install`.

## Install the agent skill

Run this from a project to install Hyfa's bundled usage instructions for
agents that read `.agents/skills`:

```bash
hyfa skill install
```

The command creates `.agents/skills/hyfa/SKILL.md` in the current directory.
It runs offline, needs no GitHub login, and copies the skill embedded in the
binary. The installed skill covers recommendations, Issue creation,
assignment, comments, dependencies, pending changes, and graph generation.

Select specific providers or another project explicitly:

```bash
hyfa skill providers
hyfa skill install --providers codex,claude-code --project-root /path/to/project
```

Providers sharing `.agents/skills` use one physical copy. Claude Code uses
`.claude/skills/hyfa`. Existing installations are preserved unless you pass
`--force`, which replaces the existing Hyfa skill directory. Installation
prints the actual destinations.

User scope is available for providers with dedicated user directories, for
example `hyfa skill install --scope user --providers claude-code`. In this
release, user scope is rejected for shared-path providers such as Codex and
`universal` because the installer's user mapping does not match their discovery
paths. Use project scope for those providers.

## Connect to GitHub

Hyfa calls GitHub's API directly from Rust. Choose one of the following ways
to authenticate.

### Sign in with a browser

```bash
hyfa auth login
hyfa auth status
```

Open the verification address printed by Hyfa and enter the displayed code.
Hyfa validates the resulting token and saves it in the operating system's
secure credential store: macOS Keychain or a Linux Secret Service session.
It never falls back to a plaintext credential file.

On GitHub.com, Hyfa uses its official OAuth App by default. Each user
authorizes that app; users do not need to register their own. An organization
that restricts OAuth apps may require its owner's approval before organization
repositories can be accessed.

To use a custom app, pass its public ID with `--client-id` or set
`HYFA_GITHUB_CLIENT_ID`. The command-line flag takes precedence over the
environment variable, which takes precedence over the official GitHub.com
default. An invalid or empty override is rejected. No Client Secret is needed.

Hyfa reuses its saved login on later commands. The official app is configured
to issue tokens without a scheduled expiration. You may need to sign in again
if access is revoked or GitHub invalidates the token.

If a custom app issues an expiring token, Hyfa records its expiration and asks
you to log in again after it expires. This release does not refresh OAuth
tokens automatically.

### Save an existing token

Pipe a token from your password manager or another trusted secret source into
`hyfa auth login --with-token`. The command only accepts piped standard input,
so the token does not need to appear in a command argument or shell history.
This method needs no OAuth App registration and uses the same secure store.

### Use an environment token

Supply a GitHub token through `GH_TOKEN` in your shell or CI secret
configuration. This works without a desktop keychain.

Credential precedence is **`GH_TOKEN` → Hyfa's saved login**.
The account must be able to read the repository; mutation commands also need
permission to make the requested changes. Then run:

```bash
hyfa sync --repo OWNER/REPO
hyfa next --repo OWNER/REPO
```

Use `hyfa auth status --json` to inspect the active account and credential
source without exposing the token. `hyfa auth logout` removes only Hyfa's
saved credential; it does not revoke the token on GitHub or modify `GH_TOKEN`.

For GitHub Enterprise, pass `--hostname github.example.com` to auth commands
and set both `HYFA_GITHUB_HOST=github.example.com` and
`HYFA_GITHUB_API_URL=https://github.example.com/api/v3/` for repository commands.
Browser login on an Enterprise host requires a Client ID from an OAuth App
registered on that instance with Device Flow enabled; Hyfa's GitHub.com app
is not used on other hosts. See
[GitHub's registration guide](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app).
Credentials are stored separately for each host. Set `HYFA_NO_KEYRING=1`
to disable secure-store discovery in headless sessions or isolated tests;
`GH_TOKEN` remains available.

The first successful synchronization creates the local snapshot used for
offline analysis. See the [usage guide](usage.md) for assignment scope,
priorities, offline edits, and graph generation.
