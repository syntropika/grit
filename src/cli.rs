use std::env;

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::{
    auth::{AuthError, AuthToken},
    github::{GitHubClient, GitHubError, HashInput},
    model::{LocalReplica, REPLICA_SCHEMA_VERSION},
    repository::{Repository, RepositoryError},
    store::{ReplicaStore, StoreError},
};

const SYNC_SCHEMA_VERSION: &str = "grit.sync/v1";

#[derive(Parser)]
#[command(name = "grit", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Synchronize one GitHub Repository into the Local replica.
    Sync {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn execute() -> Result<(), CliError> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sync { repo, json } => sync(&Repository::parse(&repo)?, json),
    }
}

fn sync(repository: &Repository, json: bool) -> Result<(), CliError> {
    let base_url = api_base_url()?;
    let hostname = env::var("GRIT_GITHUB_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| authentication_hostname(&base_url));
    let token = AuthToken::discover(&hostname)?;
    let client = GitHubClient::new(base_url, &token)?;
    let data = client.fetch_repository(repository)?;

    let hash_input = HashInput {
        schema_version: REPLICA_SCHEMA_VERSION,
        repository: repository.full_name(),
        issues: &data.issues,
        dependencies: &data.dependencies,
    };
    let canonical = serde_json::to_vec(&hash_input).map_err(CliError::EncodeHashInput)?;
    let input_hash = hex::encode(Sha256::digest(canonical));
    let replica = LocalReplica {
        schema_version: REPLICA_SCHEMA_VERSION.to_owned(),
        repository: repository.full_name().to_owned(),
        synced_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        input_hash,
        issues: data.issues,
        dependencies: data.dependencies,
    };

    ReplicaStore::discover(repository)?.publish(&replica)?;
    print_result(&replica, json)?;
    Ok(())
}

fn print_result(replica: &LocalReplica, json: bool) -> Result<(), CliError> {
    let comment_count = replica
        .issues
        .iter()
        .map(|issue| issue.comments.len())
        .sum();
    let snapshot = SnapshotSummary {
        schema_version: &replica.schema_version,
        synced_at: &replica.synced_at,
        input_hash: &replica.input_hash,
        issue_count: replica.issues.len(),
        comment_count,
        dependency_count: replica.dependencies.len(),
    };
    if json {
        let output = SyncOutput {
            schema_version: SYNC_SCHEMA_VERSION,
            command: "sync",
            repository: &replica.repository,
            snapshot,
        };
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!(
            "Synchronized {}: {} Issues, {} comments, {} Dependencies (synced_at {})",
            replica.repository,
            snapshot.issue_count,
            snapshot.comment_count,
            snapshot.dependency_count,
            snapshot.synced_at
        );
    }
    Ok(())
}

fn api_base_url() -> Result<Url, CliError> {
    let raw =
        env::var("GRIT_GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com/".to_owned());
    let mut parsed = Url::parse(&raw).map_err(CliError::ParseApiBase)?;
    if parsed.cannot_be_a_base()
        || parsed.host_str().is_none()
        || !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(CliError::InvalidApiBase);
    }
    if !parsed.path().ends_with('/') {
        let path = format!("{}/", parsed.path());
        parsed.set_path(&path);
    }
    Ok(parsed)
}

fn authentication_hostname(base_url: &Url) -> String {
    match base_url
        .host_str()
        .expect("validated API base URL has a host")
    {
        host if host.eq_ignore_ascii_case("api.github.com") => "github.com".to_owned(),
        host => host.to_owned(),
    }
}

#[derive(Serialize)]
struct SyncOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    repository: &'a str,
    snapshot: SnapshotSummary<'a>,
}

#[derive(Serialize)]
struct SnapshotSummary<'a> {
    schema_version: &'a str,
    synced_at: &'a str,
    input_hash: &'a str,
    issue_count: usize,
    comment_count: usize,
    dependency_count: usize,
}

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("GRIT_GITHUB_API_URL is invalid: {0}")]
    ParseApiBase(url::ParseError),
    #[error("GRIT_GITHUB_API_URL must be a safe absolute HTTP(S) base URL")]
    InvalidApiBase,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("could not encode normalized input for hashing: {0}")]
    EncodeHashInput(serde_json::Error),
    #[error("could not encode sync JSON output: {0}")]
    EncodeOutput(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::authentication_hostname;
    use url::Url;

    #[test]
    fn public_github_api_uses_the_github_dot_com_authentication_host() {
        let public_api = Url::parse("https://api.github.com/").expect("public API URL");
        let enterprise_api =
            Url::parse("https://github.example/api/v3/").expect("enterprise API URL");

        assert_eq!(authentication_hostname(&public_api), "github.com");
        assert_eq!(authentication_hostname(&enterprise_api), "github.example");
    }
}
