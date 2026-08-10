use std::{env, path::PathBuf};

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::{
    auth::{AuthError, AuthToken},
    github::{CreateLabelRequest, GitHubClient, GitHubError, LabelCreation},
    graph::{ARTIFACT_SCHEMA_VERSION, GraphError, publish_site},
    model::{LocalReplica, ReplicaError},
    operational::{ExecutionScope, PreparedRepository, analyze_ready},
    plan::{DependencyLayers, PlanIssue},
    priority::{
        DeclaredPriority, PriorityState, missing_canonical_labels, present_canonical_labels,
    },
    ranking::{self, NextAnalysis, PlanDecision},
    repository::{Repository, RepositoryError},
    store::{ReplicaStore, StoreError},
};

const SYNC_SCHEMA_VERSION: &str = "grit.sync/v1";
const READY_SCHEMA_VERSION: &str = "grit.ready/v1";
const GRAPH_SCHEMA_VERSION: &str = "grit.graph/v1";
const INIT_SCHEMA_VERSION: &str = "grit.init/v1";
const PLAN_SCHEMA_VERSION: &str = "grit.plan/v1";

#[derive(Parser)]
#[command(name = "grit", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a deterministic static Issue graph site.
    Graph {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Target directory for the complete static site.
        #[arg(long)]
        output: PathBuf,
        /// Select Ready work assigned to this GitHub login.
        #[arg(long)]
        assignee: Option<String>,
        /// Ranking horizon embedded in the static analysis.
        #[arg(long, default_value_t = ranking::DEFAULT_HORIZON)]
        horizon: u8,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Recommend the best executable first step under next/v1.
    Next {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Select Ready work assigned to this GitHub login.
        #[arg(long)]
        assignee: Option<String>,
        /// Number of completions to evaluate, from one through the default three.
        #[arg(long, default_value_t = ranking::DEFAULT_HORIZON)]
        horizon: u8,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Explain the best rollout and structural dependency layers.
    Plan {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Select Ready work assigned to this GitHub login.
        #[arg(long)]
        assignee: Option<String>,
        /// Number of completions to evaluate, from one through the default three.
        #[arg(long, default_value_t = ranking::DEFAULT_HORIZON)]
        horizon: u8,
        /// Capacity is intentionally unsupported by plan/v1.
        #[arg(long)]
        workers: Option<usize>,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Create any missing canonical Priority labels.
    Init {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
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
        Command::Graph {
            repo,
            output,
            assignee,
            horizon,
            json,
        } => graph(
            &Repository::parse(&repo)?,
            &output,
            assignee.as_deref(),
            horizon,
            json,
        ),
        Command::Next {
            repo,
            assignee,
            horizon,
            json,
        } => next(
            &Repository::parse(&repo)?,
            assignee.as_deref(),
            horizon,
            json,
        ),
        Command::Plan {
            repo,
            assignee,
            horizon,
            workers,
            json,
        } => plan(
            &Repository::parse(&repo)?,
            assignee.as_deref(),
            horizon,
            workers,
            json,
        ),
        Command::Init { repo, json } => initialize(&Repository::parse(&repo)?, json),
        Command::Sync { repo, json } => sync(&Repository::parse(&repo)?, json),
        Command::Ready {
            repo,
            assignee,
            json,
        } => ready(&Repository::parse(&repo)?, assignee.as_deref(), json),
    }
}

fn graph(
    repository: &Repository,
    output: &std::path::Path,
    assignee: Option<&str>,
    horizon: u8,
    json: bool,
) -> Result<(), CliError> {
    if !(ranking::MIN_HORIZON..=ranking::MAX_HORIZON).contains(&horizon) {
        return Err(CliError::UnsupportedNextHorizon(horizon));
    }
    let (replica, source) = refresh_or_local(repository)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let site = publish_site(&replica, scope, horizon, output)?;
    let output_path = output.display().to_string();
    let result = GraphOutput {
        schema_version: GRAPH_SCHEMA_VERSION,
        command: "graph",
        repository: &replica.repository,
        source,
        synced_at: &replica.synced_at,
        input_hash: &replica.input_hash,
        output: &output_path,
        artifact: GraphArtifactSummary {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            node_count: site.node_count,
            edge_count: site.edge_count,
            artifact_hash: &site.artifact_hash,
        },
    };
    if json {
        serde_json::to_writer(std::io::stdout().lock(), &result).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!(
            "Generated {} nodes and {} Dependencies in {}",
            site.node_count, site.edge_count, output_path
        );
        if source.is_fallback() {
            eprintln!(
                "warning: GitHub refresh failed; generated from Local replica at {}",
                replica.synced_at
            );
        }
    }
    Ok(())
}

fn plan(
    repository: &Repository,
    assignee: Option<&str>,
    horizon: u8,
    workers: Option<usize>,
    json: bool,
) -> Result<(), CliError> {
    if workers.is_some() {
        return Err(CliError::UnsupportedPlanWorkers);
    }
    if !(ranking::MIN_HORIZON..=ranking::MAX_HORIZON).contains(&horizon) {
        return Err(CliError::UnsupportedNextHorizon(horizon));
    }
    let (replica, source) = refresh_or_local(repository)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let prepared = PreparedRepository::prepare(&replica);
    let analysis = ranking::analyze_prepared_bundle(&prepared, scope, horizon);
    let structural = crate::plan::analyze_with_ready(prepared.graph(), scope, &analysis.ready);
    let decision = analysis.next.into_plan_decision();
    let parallel_now = structural.parallel_now;
    let dependency_layers = structural.dependency_layers;
    let warnings = analysis_warnings(&replica, source);

    if json {
        let output = PlanOutput {
            schema_version: PLAN_SCHEMA_VERSION,
            policy_version: ranking::POLICY_VERSION,
            command: "plan",
            repository: &replica.repository,
            source,
            synced_at: &replica.synced_at,
            replica_snapshot_hash: &replica.input_hash,
            execution_scope: execution_scope_output(assignee),
            decision,
            parallel_now,
            dependency_layers,
            warnings,
        };
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!(
            "plan/v1 for {} (synced_at {}):",
            replica.repository, replica.synced_at
        );
        match decision.human_recommendation_summary() {
            Some(recommendation) => println!("{recommendation}"),
            None => println!("{}", decision.summary().human_empty_summary()),
        }
        println!("parallel_now:");
        for issue in &parallel_now {
            println!("#{} {}", issue.number, issue.title);
        }
        println!("dependency layers (counterfactual topology):");
        for layer in dependency_layers.human_lines() {
            println!("{layer}");
        }
        if let Some(warning) = decision.truncation_warning() {
            eprintln!("warning: {warning}");
        }
        for warning in &warnings {
            print_warning(warning);
        }
    }
    Ok(())
}

fn initialize(repository: &Repository, json: bool) -> Result<(), CliError> {
    let client = github_client()?;
    let labels = client.fetch_labels(repository)?;
    let mut already_present: Vec<_> = present_canonical_labels(&labels)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut created_labels = Vec::new();
    for priority in DeclaredPriority::ALL {
        let spec = priority.spec();
        if labels
            .iter()
            .any(|label| label.name.eq_ignore_ascii_case(spec.name))
        {
            continue;
        }
        let request = CreateLabelRequest::new(spec.name, spec.color, spec.description);
        match client.create_label(repository, &request)? {
            LabelCreation::Created => created_labels.push(spec.name.to_owned()),
            LabelCreation::AlreadyPresent => already_present.push(spec.name.to_owned()),
        }
    }
    already_present.sort();
    let output = InitOutput {
        schema_version: INIT_SCHEMA_VERSION,
        command: "init",
        repository: repository.full_name(),
        created_labels,
        already_present,
    };
    if json {
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else if output.created_labels.is_empty() {
        println!(
            "Priority labels are already initialized in {}",
            repository.full_name()
        );
    } else {
        println!(
            "Created {} in {}",
            output.created_labels.join(", "),
            repository.full_name()
        );
    }
    Ok(())
}
fn sync(repository: &Repository, json: bool) -> Result<(), CliError> {
    let replica = synchronize(repository)?;
    print_sync_result(&replica, json)?;
    Ok(())
}

fn next(
    repository: &Repository,
    assignee: Option<&str>,
    horizon: u8,
    json: bool,
) -> Result<(), CliError> {
    if !(ranking::MIN_HORIZON..=ranking::MAX_HORIZON).contains(&horizon) {
        return Err(CliError::UnsupportedNextHorizon(horizon));
    }
    let (replica, source) = refresh_or_local(repository)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let analysis = ranking::analyze(&replica, scope, horizon);
    let warnings = analysis_warnings(&replica, source);
    if json {
        let output = NextOutput {
            schema_version: ranking::OUTPUT_SCHEMA_VERSION,
            policy_version: ranking::POLICY_VERSION,
            command: "next",
            repository: &replica.repository,
            source,
            synced_at: &replica.synced_at,
            replica_snapshot_hash: &replica.input_hash,
            execution_scope: execution_scope_output(assignee),
            analysis,
            warnings,
        };
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!(
            "next/v1 recommendation in {} (synced_at {}):",
            replica.repository, replica.synced_at
        );
        match analysis.human_recommendation_summary() {
            Some(recommendation) => println!("{recommendation}"),
            None => println!("{}", analysis.summary().human_empty_summary()),
        }
        if let Some(warning) = analysis.truncation_warning() {
            eprintln!("warning: {warning}");
        }
        for warning in &warnings {
            print_warning(warning);
        }
    }
    Ok(())
}

fn synchronize(repository: &Repository) -> Result<LocalReplica, CliError> {
    let client = github_client()?;
    let data = client.fetch_repository(repository)?;

    let replica = LocalReplica::build(
        repository.full_name().to_owned(),
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        data.labels,
        data.issues,
        data.dependencies,
    )?;

    ReplicaStore::discover(repository)?.publish(&replica)?;
    Ok(replica)
}

fn github_client() -> Result<GitHubClient, CliError> {
    let base_url = api_base_url()?;
    let hostname = env::var("GRIT_GITHUB_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| authentication_hostname(&base_url));
    let token = AuthToken::discover(&hostname)?;
    GitHubClient::new(base_url, &token).map_err(Into::into)
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
    let (replica, source) = refresh_or_local(repository)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let analysis = analyze_ready(&replica, scope);
    let warnings = analysis_warnings(&replica, source);
    let issues: Vec<_> = analysis
        .executable
        .iter()
        .map(|issue| ReadyIssue {
            number: issue.number,
            url: &issue.url,
            title: &issue.title,
            ready: true,
            available: issue.assignees.is_empty(),
            priority: PriorityState::from_issue_labels(&issue.labels),
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
        execution_scope: execution_scope_output(assignee),
        issues,
        summary: ReadySummary {
            operational_issue_count: analysis.operational_issue_count,
            ready_count: analysis.ready_count,
            executable_count: analysis.executable.len(),
            assigned_ready_count: analysis.assigned_ready_count,
            blocked_count: analysis.blocked_count,
        },
        warnings,
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
            println!(
                "#{} {} [{}]",
                issue.number,
                issue.title,
                issue.priority.display_name()
            );
        }
        for warning in &output.warnings {
            print_warning(warning);
        }
    }
    Ok(())
}

fn refresh_or_local(repository: &Repository) -> Result<(LocalReplica, ReplicaSource), CliError> {
    match synchronize(repository) {
        Ok(replica) => Ok((replica, ReplicaSource::Live)),
        Err(refresh_error) => {
            let store = ReplicaStore::discover(repository)?;
            match store.load(repository) {
                Ok(replica) => Ok((replica, ReplicaSource::LocalFallback)),
                Err(replica_error) => Err(CliError::RefreshAndReplicaUnavailable {
                    refresh: refresh_error.to_string(),
                    replica: replica_error.to_string(),
                }),
            }
        }
    }
}

fn analysis_warnings(replica: &LocalReplica, source: ReplicaSource) -> Vec<ReadyWarning> {
    let mut warnings = Vec::new();
    if let Some(repository_labels) = replica.repository_labels.as_deref() {
        let missing_labels: Vec<_> = missing_canonical_labels(repository_labels)
            .into_iter()
            .map(str::to_owned)
            .collect();
        if !missing_labels.is_empty() {
            warnings.push(ReadyWarning {
                code: "missing_priority_labels",
                message: "Repository is missing canonical Priority labels".to_owned(),
                issue_number: None,
                labels: missing_labels,
            });
        }
    }
    for issue in replica
        .issues
        .iter()
        .filter(|issue| issue.state.eq_ignore_ascii_case("open"))
    {
        let priority = PriorityState::from_issue_labels(&issue.labels);
        if let Some(labels) = priority.conflict_labels() {
            warnings.push(ReadyWarning {
                code: "priority_conflict",
                message: format!(
                    "Issue #{} has multiple canonical Priority labels",
                    issue.number
                ),
                issue_number: Some(issue.number),
                labels: labels.to_vec(),
            });
        }
    }
    if let Some(warning) = source.warning() {
        warnings.push(warning);
    }
    warnings
}

fn execution_scope_output(assignee: Option<&str>) -> ExecutionScopeOutput<'_> {
    match assignee {
        Some(assignee) => ExecutionScopeOutput {
            mode: "assignee",
            assignee: Some(assignee),
        },
        None => ExecutionScopeOutput {
            mode: "available",
            assignee: None,
        },
    }
}

fn print_warning(warning: &ReadyWarning) {
    if warning.labels.is_empty() {
        eprintln!("warning: {}", warning.message);
    } else {
        eprintln!(
            "warning: {} ({})",
            warning.message,
            warning.labels.join(", ")
        );
    }
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
struct GraphOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    repository: &'a str,
    source: ReplicaSource,
    synced_at: &'a str,
    input_hash: &'a str,
    output: &'a str,
    artifact: GraphArtifactSummary<'a>,
}

#[derive(Serialize)]
struct GraphArtifactSummary<'a> {
    schema_version: &'static str,
    node_count: usize,
    edge_count: usize,
    artifact_hash: &'a str,
}

#[derive(Serialize)]
struct InitOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    repository: &'a str,
    created_labels: Vec<String>,
    already_present: Vec<String>,
}

#[derive(Serialize)]
struct NextOutput<'a> {
    schema_version: &'static str,
    policy_version: &'static str,
    command: &'static str,
    repository: &'a str,
    source: ReplicaSource,
    synced_at: &'a str,
    replica_snapshot_hash: &'a str,
    execution_scope: ExecutionScopeOutput<'a>,
    #[serde(flatten)]
    analysis: NextAnalysis,
    warnings: Vec<ReadyWarning>,
}

#[derive(Serialize)]
struct PlanOutput<'a> {
    schema_version: &'static str,
    policy_version: &'static str,
    command: &'static str,
    repository: &'a str,
    source: ReplicaSource,
    synced_at: &'a str,
    replica_snapshot_hash: &'a str,
    execution_scope: ExecutionScopeOutput<'a>,
    decision: PlanDecision,
    parallel_now: Vec<PlanIssue>,
    dependency_layers: DependencyLayers,
    warnings: Vec<ReadyWarning>,
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
    priority: PriorityState,
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
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_number: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
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
            message: "GitHub refresh failed; using the latest valid Local replica".to_owned(),
            issue_number: None,
            labels: Vec::new(),
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
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("could not encode command JSON output: {0}")]
    EncodeOutput(serde_json::Error),
    #[error("next/v1 horizon must be between 1 and 3, not {0}")]
    UnsupportedNextHorizon(u8),
    #[error("grit plan does not accept --workers in v1")]
    UnsupportedPlanWorkers,
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
