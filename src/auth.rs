//! Native authentication with host-scoped secure storage.
mod device;
mod http;
mod store;
#[cfg(test)]
mod tests;

use std::{
    env,
    io::{self, IsTerminal, Read},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Subcommand;
use serde::Serialize;
use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

use device::{DeviceApi, SystemClock};
use http::GitHubAuth;
use store::{CredentialStore, OsCredentialStore};

pub(crate) struct AuthToken {
    value: Zeroizing<String>,
    expires_at: Option<u64>,
}

impl AuthToken {
    pub(crate) fn discover(hostname: &str, api_url: &Url) -> Result<Self, AuthError> {
        let host = Host::parse(hostname)?;
        // A saved credential is valid only for its host's canonical HTTPS API.
        // Explicit environment tokens retain API-override support for tests and proxies.
        let use_store = host.matches_api(api_url);
        let found = discover_with(env::var("GH_TOKEN").ok(), || {
            if use_store {
                OsCredentialStore.get(&host)
            } else {
                Ok(None)
            }
        })?;
        Ok(found.token)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.value
    }

    fn with_lifetime(mut self, lifetime: Option<u64>) -> Result<Self, AuthError> {
        self.expires_at = lifetime
            .map(|seconds| {
                now_seconds()
                    .checked_add(seconds)
                    .ok_or(AuthError::Response)
            })
            .transpose()?;
        self.ensure_current()?;
        Ok(self)
    }

    fn ensure_current(&self) -> Result<(), AuthError> {
        if self
            .expires_at
            .is_some_and(|expiry| expiry <= now_seconds())
        {
            Err(AuthError::CredentialExpired)
        } else {
            Ok(())
        }
    }

    fn parse(value: impl AsRef<str>) -> Result<Self, AuthError> {
        let token = value.as_ref().trim();
        if token.is_empty()
            || token.len() > 16_384
            || token.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(AuthError::InvalidToken);
        }
        Ok(Self {
            value: Zeroizing::new(token.to_owned()),
            expires_at: None,
        })
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Host(String);

impl Host {
    fn parse(value: &str) -> Result<Self, AuthError> {
        let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
        if value.is_empty()
            || value.len() > 253
            || value.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(AuthError::InvalidHost);
        }
        Ok(Self(value))
    }

    fn api_url(&self) -> Url {
        let base = if self.0 == "github.com" {
            "https://api.github.com/".to_owned()
        } else {
            format!("https://{}/api/v3/", self.0)
        };
        Url::parse(&base).expect("validated hostname forms an HTTPS URL")
    }

    fn matches_api(&self, candidate: &Url) -> bool {
        let expected = self.api_url();
        candidate.scheme() == "https"
            && candidate.host_str() == expected.host_str()
            && candidate.port_or_known_default() == Some(443)
            && candidate.path() == expected.path()
            && candidate.username().is_empty()
            && candidate.password().is_none()
            && candidate.query().is_none()
            && candidate.fragment().is_none()
    }
}

#[derive(Subcommand)]
pub(crate) enum AuthCommand {
    /// Sign in with a browser device code or a token read from standard input.
    Login {
        /// GitHub hostname, defaulting to GRIT_GITHUB_HOST or github.com.
        #[arg(long)]
        hostname: Option<String>,
        /// Your GitHub OAuth app's public client ID; also reads GRIT_GITHUB_CLIENT_ID.
        #[arg(long, conflicts_with = "with_token")]
        client_id: Option<String>,
        /// Read a token from piped standard input and save it in the OS credential store.
        #[arg(long)]
        with_token: bool,
    },
    /// Verify the active credential and show its account and source without exposing it.
    Status {
        #[arg(long)]
        hostname: Option<String>,
        /// Emit versioned machine-readable output without credentials.
        #[arg(long)]
        json: bool,
    },
    /// Remove Grit's saved credential for this host; leave GH_TOKEN unchanged.
    Logout {
        #[arg(long)]
        hostname: Option<String>,
    },
}

pub(crate) fn execute(command: AuthCommand) -> Result<(), AuthError> {
    match command {
        AuthCommand::Login {
            hostname,
            client_id,
            with_token,
        } => {
            let host = configured_host(hostname)?;
            let client_id = if with_token {
                None
            } else {
                Some(configured_client_id(client_id)?)
            };
            // Fail before asking for browser authorization when secure persistence is unavailable.
            OsCredentialStore.check(&host)?;
            let api = GitHubAuth::new(&host)?;
            let token = if let Some(client_id) = client_id {
                device::authorize(
                    &api,
                    &mut SystemClock::new(),
                    &host,
                    &client_id,
                    |challenge| {
                        eprintln!(
                            "Open {} in your browser and enter code {}.",
                            challenge.verification_uri, challenge.user_code
                        );
                        eprintln!(
                            "Waiting for GitHub authorization (up to {} seconds)...",
                            challenge.expires_in
                        );
                    },
                )?
            } else {
                read_stdin_token(io::stdin().is_terminal(), io::stdin().lock())?
            };
            let account = verify_and_save(&api, &OsCredentialStore, &host, &token)?;
            println!(
                "Signed in to {} as {}. Credential saved in the OS secure store.",
                host.0, account
            );
            if token.expires_at.is_some() {
                eprintln!(
                    "GitHub issued an expiring credential. Run grit auth login again when it expires; automatic refresh is not supported."
                );
            }
            if env::var("GH_TOKEN").is_ok_and(|value| !value.trim().is_empty()) {
                eprintln!(
                    "Note: GH_TOKEN takes precedence over this saved credential while it is set."
                );
            }
        }
        AuthCommand::Status { hostname, json } => {
            let host = configured_host(hostname)?;
            let found = discover_with(env::var("GH_TOKEN").ok(), || OsCredentialStore.get(&host))?;
            let account = GitHubAuth::new(&host)?.account(&found.token)?;
            let status = AuthStatus {
                schema_version: "grit.auth/v1",
                hostname: &host.0,
                account: &account,
                source: found.source,
                authenticated: true,
                expires_at: found.token.expires_at,
            };
            if json {
                serde_json::to_writer(io::stdout().lock(), &status)
                    .map_err(|_| AuthError::Output)?;
                println!();
            } else {
                println!(
                    "Signed in to {} as {} using {}.",
                    host.0,
                    account,
                    found.source.label()
                );
            }
        }
        AuthCommand::Logout { hostname } => {
            let host = configured_host(hostname)?;
            let removed = OsCredentialStore.delete(&host)?;
            if removed {
                println!("Removed Grit's saved credential for {}.", host.0);
            } else {
                println!("No Grit credential is saved for {}.", host.0);
            }
            println!("GH_TOKEN is unchanged. This does not revoke the token on GitHub.");
        }
    }
    Ok(())
}

fn configured_host(hostname: Option<String>) -> Result<Host, AuthError> {
    Host::parse(
        &hostname
            .or_else(|| env::var("GRIT_GITHUB_HOST").ok())
            .unwrap_or_else(|| "github.com".to_owned()),
    )
}

fn configured_client_id(client_id: Option<String>) -> Result<String, AuthError> {
    let id = client_id
        .or_else(|| env::var("GRIT_GITHUB_CLIENT_ID").ok())
        .ok_or(AuthError::MissingClientId)?;
    let id = id.trim();
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        return Err(AuthError::InvalidClientId);
    }
    Ok(id.to_owned())
}

fn read_stdin_token(terminal: bool, reader: impl Read) -> Result<AuthToken, AuthError> {
    if terminal {
        return Err(AuthError::TokenInputTerminal);
    }
    let mut value = Zeroizing::new(String::new());
    reader
        .take(16_386)
        .read_to_string(&mut value)
        .map_err(|_| AuthError::TokenInput)?;
    if value.len() > 16_384 {
        return Err(AuthError::InvalidToken);
    }
    AuthToken::parse(&value)
}

fn verify_and_save(
    api: &impl DeviceApi,
    store: &impl CredentialStore,
    host: &Host,
    token: &AuthToken,
) -> Result<String, AuthError> {
    let account = api.account(token)?;
    store.set(host, token)?;
    Ok(account)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CredentialSource {
    Environment,
    Grit,
}

impl CredentialSource {
    fn label(self) -> &'static str {
        match self {
            Self::Environment => "GH_TOKEN",
            Self::Grit => "Grit's OS credential store",
        }
    }
}

struct Discovered {
    token: AuthToken,
    source: CredentialSource,
}

fn discover_with(
    environment: Option<String>,
    saved: impl FnOnce() -> Result<Option<AuthToken>, AuthError>,
) -> Result<Discovered, AuthError> {
    if let Some(value) = environment.filter(|value| !value.trim().is_empty()) {
        return Ok(Discovered {
            token: AuthToken::parse(Zeroizing::new(value))?,
            source: CredentialSource::Environment,
        });
    }
    if let Some(token) = saved()? {
        token.ensure_current()?;
        return Ok(Discovered {
            token,
            source: CredentialSource::Grit,
        });
    }
    Err(AuthError::Unavailable)
}

#[derive(Serialize)]
struct AuthStatus<'a> {
    schema_version: &'static str,
    hostname: &'a str,
    account: &'a str,
    source: CredentialSource,
    authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

#[derive(Debug, Error)]
pub(crate) enum AuthError {
    #[error(
        "no saved Grit credential or non-empty GH_TOKEN is available; run grit auth login or set GH_TOKEN"
    )]
    Unavailable,
    #[error(
        "hostname must be a DNS name such as github.com, without a scheme, port, path, or credentials"
    )]
    InvalidHost,
    #[error(
        "a GitHub OAuth app client ID is required: use --client-id or GRIT_GITHUB_CLIENT_ID with device flow enabled; alternatively use grit auth login --with-token or GH_TOKEN"
    )]
    MissingClientId,
    #[error("the GitHub OAuth app client ID is invalid")]
    InvalidClientId,
    #[error("the token is empty or contains invalid characters")]
    InvalidToken,
    #[error(
        "--with-token requires piped standard input; do not place the token in command arguments"
    )]
    TokenInputTerminal,
    #[error("could not read the token from standard input")]
    TokenInput,
    #[error(
        "the OS secure credential store is unavailable or locked; unlock macOS Keychain or a Linux Secret Service session, or use GH_TOKEN without storing a credential"
    )]
    SecureStore,
    #[error("GitHub authentication request failed; check your connection and try again")]
    Network,
    #[error("GitHub rejected the credential; run grit auth login again or replace GH_TOKEN")]
    Rejected,
    #[error("GitHub authentication returned HTTP {0}; no credential was saved")]
    Http(u16),
    #[error("GitHub returned an invalid authentication response; no credential was saved")]
    Response,
    #[error("GitHub device authorization expired; run grit auth login again")]
    Expired,
    #[error(
        "Grit's saved GitHub credential has expired; run grit auth login again (automatic token refresh is not supported)"
    )]
    CredentialExpired,
    #[error("GitHub device authorization was denied; no credential was saved")]
    Denied,
    #[error(
        "device flow is disabled for this OAuth app; enable it in the app's GitHub settings or use --with-token"
    )]
    DeviceFlowDisabled,
    #[error("GitHub rejected the OAuth app client ID; check --client-id or GRIT_GITHUB_CLIENT_ID")]
    ClientRejected,
    #[error("GitHub could not authorize this device; run grit auth login again")]
    DeviceRejected,
    #[error("could not write authentication status output")]
    Output,
}
