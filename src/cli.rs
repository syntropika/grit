use std::env;

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::{
    auth::{AuthError, AuthToken},
    github::{GitHubClient, GitHubError},
    model::{LocalReplica, ReplicaError},
    operational::{ExecutionScope, analyze_ready},
    repository::{Repository, RepositoryError},
    store::{ReplicaStore, StoreError},
};

const SYNC_SCHEMA_VERSION: &str = "grit.sync/v1";
const READY_SCHEMA_VERSION: &str = "grit.ready/v1";

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
    /// Enumerate the complete Executable frontier.
    Ready {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Select Ready work assigned to this GitHub login.
        #[arg(long)]
        assignee: Option<String>,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn execute() -> Result<(), CliError> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sync { repo, json } => sync(&Repository::parse(&repo)?, json),
        Command::Ready {
            repo,
            assignee,
            json,
        } => ready(&Repository::parse(&repo)?, assignee.as_deref(), json),
    }
}

fn sync(repository: &Repository, json: bool) -> Result<(), CliError> {
    let replica = synchronize(repository)?;
    print_sync_result(&replica, json)?;
    Ok(())
}

fn synchronize(repository: &Repository) -> Result<LocalReplica, CliError> {
    let base_url = api_base_url()?;
    let hostname = env::var("GRIT_GITHUB_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| authentication_hostname(&base_url));
    let token = AuthToken::discover(&hostname)?;
    let client = GitHubClient::new(base_url, &token)?;
    let data = client.fetch_repository(repository)?;

    let replica = LocalReplica::build(
        repository.full_name().to_owned(),
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        data.issues,
        data.dependencies,
    )?;

    ReplicaStore::discover(repository)?.publish(&replica)?;
    Ok(replica)
}

fn print_sync_result(replica: &LocalReplica, json: bool) -> Result<(), CliError> {
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

fn ready(repository: &Repository, assignee: Option<&str>, json: bool) -> Result<(), CliError> {
    let (replica, source) = match synchronize(repository) {
        Ok(replica) => (replica, ReplicaSource::Live),
        Err(refresh_error) => {
            let store = ReplicaStore::discover(repository)?;
            match store.load(repository) {
                Ok(replica) => (replica, ReplicaSource::LocalFallback),
                Err(replica_error) => {
                    return Err(CliError::RefreshAndReplicaUnavailable {
                        refresh: refresh_error.to_string(),
                        replica: replica_error.to_string(),
                    });
                }
            }
        }
    };
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let analysis = analyze_ready(&replica, scope);
    let issues: Vec<_> = analysis
        .executable
        .iter()
        .map(|issue| ReadyIssue {
            number: issue.number,
            url: &issue.url,
            title: &issue.title,
            ready: true,
            available: issue.assignees.is_empty(),
            assignees: issue
                .assignees
                .iter()
                .map(|actor| actor.login.as_str())
                .collect(),
        })
        .collect();
    let output = ReadyOutput {
        schema_version: READY_SCHEMA_VERSION,
        command: "ready",
        repository: &replica.repository,
        source,
        synced_at: &replica.synced_at,
        input_hash: &replica.input_hash,
        execution_scope: match assignee {
            Some(assignee) => ExecutionScopeOutput {
                mode: "assignee",
                assignee: Some(assignee),
            },
            None => ExecutionScopeOutput {
                mode: "available",
                assignee: None,
            },
        },
        issues,
        summary: ReadySummary {
            operational_issue_count: analysis.operational_issue_count,
            ready_count: analysis.ready_count,
            executable_count: analysis.executable.len(),
            assigned_ready_count: analysis.assigned_ready_count,
            blocked_count: analysis.blocked_count,
        },
        warnings: source.warning().into_iter().collect(),
    };
    if json {
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!(
            "Executable Issues in {} (synced_at {}):",
            replica.repository, replica.synced_at
        );
        for issue in &output.issues {
            println!("#{} {}", issue.number, issue.title);
        }
        if source.is_fallback() {
            eprintln!(
                "warning: GitHub refresh failed; using Local replica from {}",
                replica.synced_at
            );
        }
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

#[derive(Serialize)]
struct ReadyOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    repository: &'a str,
    source: ReplicaSource,
    synced_at: &'a str,
    input_hash: &'a str,
    execution_scope: ExecutionScopeOutput<'a>,
    issues: Vec<ReadyIssue<'a>>,
    summary: ReadySummary,
    warnings: Vec<ReadyWarning>,
}

#[derive(Serialize)]
struct ExecutionScopeOutput<'a> {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignee: Option<&'a str>,
}

#[derive(Serialize)]
struct ReadyIssue<'a> {
    number: u64,
    url: &'a str,
    title: &'a str,
    ready: bool,
    available: bool,
    assignees: Vec<&'a str>,
}

#[derive(Serialize)]
struct ReadySummary {
    operational_issue_count: usize,
    ready_count: usize,
    executable_count: usize,
    assigned_ready_count: usize,
    blocked_count: usize,
}

#[derive(Serialize)]
struct ReadyWarning {
    code: &'static str,
    message: &'static str,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReplicaSource {
    Live,
    LocalFallback,
}

impl ReplicaSource {
    fn is_fallback(self) -> bool {
        matches!(self, Self::LocalFallback)
    }

    fn warning(self) -> Option<ReadyWarning> {
        self.is_fallback().then_some(ReadyWarning {
            code: "offline_fallback",
            message: "GitHub refresh failed; using the latest valid Local replica",
        })
    }
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
    #[error(transparent)]
    Replica(#[from] ReplicaError),
    #[error("could not encode command JSON output: {0}")]
    EncodeOutput(serde_json::Error),
    #[error("GitHub refresh failed ({refresh}); no valid Local replica is available ({replica})")]
    RefreshAndReplicaUnavailable { refresh: String, replica: String },
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
