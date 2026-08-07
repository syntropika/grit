use std::env;

use clap::{Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::{
    auth::{AuthError, AuthToken},
    github::{CreateLabelRequest, GitHubClient, GitHubError, LabelCreation},
    model::{LocalReplica, ReplicaError},
    operational::{ExecutionScope, analyze_ready},
    outbox::{OutboxError, OutboxStore, PendingMutation},
    priority::{
        DeclaredPriority, LogicalPriority, PrioritySelection, PriorityState,
        missing_canonical_labels, present_canonical_labels,
    },
    priority_update::{self, PendingPriorityUpdateError, PriorityUpdateError},
    ranking::{self, NextAnalysis},
    replica_sync::{self, ReplicaSyncError},
    repository::{IssueReference, IssueReferenceError, Repository, RepositoryError},
    store::{ReplicaStore, StoreError},
    working_graph::{PendingProvenance, WorkingGraph, WorkingGraphError},
};

const SYNC_SCHEMA_VERSION: &str = "grit.sync/v1";
const READY_SCHEMA_VERSION: &str = "grit.ready/v1";
const INIT_SCHEMA_VERSION: &str = "grit.init/v1";
const PRIORITY_UPDATE_SCHEMA_VERSION: &str = "grit.priority-update/v1";

#[derive(Parser)]
#[command(name = "grit", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Recommend the best executable first step under next/v1.
    Next {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Select Ready work assigned to this GitHub login.
        #[arg(long)]
        assignee: Option<String>,
        /// Number of completions to evaluate; this slice implements exactly one.
        #[arg(long, default_value_t = ranking::HORIZON)]
        horizon: u8,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Update one Issue's logical Declared priority.
    Update {
        /// Issue in OWNER/REPO#NUMBER form.
        issue: String,
        /// Desired logical Priority, or none to remove it.
        #[arg(long)]
        priority: PrioritySelection,
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
        Command::Update {
            issue,
            priority,
            json,
        } => update_priority(&issue, priority, json),
        Command::Init { repo, json } => initialize(&Repository::parse(&repo)?, json),
        Command::Sync { repo, json } => sync(&Repository::parse(&repo)?, json),
        Command::Ready {
            repo,
            assignee,
            json,
        } => ready(&Repository::parse(&repo)?, assignee.as_deref(), json),
    }
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

fn update_priority(issue: &str, requested: PrioritySelection, json: bool) -> Result<(), CliError> {
    let issue = IssueReference::parse(issue)?;
    let client = match github_client() {
        Ok(client) => client,
        Err(CliError::Auth(source)) => {
            return queue_priority_update(&issue, requested, source.to_string(), json);
        }
        Err(error) => return Err(error),
    };
    let result = match priority_update::update(&client, &issue, requested) {
        Ok(result) => result,
        Err(source) if source.permits_offline_queue() => {
            return queue_priority_update(&issue, requested, source.to_string(), json);
        }
        Err(source) => return Err(source.into()),
    };
    let output = PriorityUpdateOutput {
        schema_version: PRIORITY_UPDATE_SCHEMA_VERSION,
        command: "update",
        status: PriorityUpdateStatus::Synchronized,
        pending: false,
        repository: &result.replica.repository,
        issue: PriorityIssueOutput {
            key: &result.issue_key,
            number: result.issue_number,
            url: &result.issue_url,
        },
        previous_priority: result.previous_priority,
        resulting_priority: result.resulting_priority,
        operation: None,
        working_graph: None,
        snapshot: snapshot_summary(&result.replica),
    };
    print_priority_update(output, json)
}

fn queue_priority_update(
    issue: &IssueReference,
    requested: PrioritySelection,
    online_failure: String,
    json: bool,
) -> Result<(), CliError> {
    let result = priority_update::queue(issue, requested).map_err(|queue| {
        CliError::OnlinePriorityUpdateAndQueueFailed {
            online: online_failure,
            queue,
        }
    })?;
    let output = PriorityUpdateOutput {
        schema_version: PRIORITY_UPDATE_SCHEMA_VERSION,
        command: "update",
        status: PriorityUpdateStatus::Pending,
        pending: true,
        repository: &result.replica.repository,
        issue: PriorityIssueOutput {
            key: &result.issue_key,
            number: result.issue_number,
            url: &result.issue_url,
        },
        previous_priority: result.previous_priority,
        resulting_priority: result.resulting_priority,
        operation: Some(PendingOperationOutput::from(&result.operation)),
        working_graph: Some(WorkingGraphSummary {
            input_hash: &result.working_input_hash,
        }),
        snapshot: snapshot_summary(&result.replica),
    };
    print_priority_update(output, json)
}

fn print_priority_update(output: PriorityUpdateOutput<'_>, json: bool) -> Result<(), CliError> {
    if json {
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else if matches!(output.status, PriorityUpdateStatus::Pending) {
        println!(
            "Queued {} Priority from {} to {} as Pending mutation {}",
            output.issue.key,
            output.previous_priority.display_name(),
            output.resulting_priority.display_name(),
            output
                .operation
                .as_ref()
                .expect("Pending output includes an operation")
                .id
        );
    } else {
        println!(
            "Updated {} Priority from {} to {}",
            output.issue.key,
            output.previous_priority.display_name(),
            output.resulting_priority.display_name()
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
    if horizon != ranking::HORIZON {
        return Err(CliError::UnsupportedNextHorizon(horizon));
    }
    let (replica, source) = refresh_or_local(repository)?;
    let outbox = OutboxStore::discover(repository)?.load(repository)?;
    let working = WorkingGraph::project(&replica, &outbox)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let analysis = ranking::analyze(&working, scope);
    let warnings = analysis_warnings(&working, source);
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
        match analysis.recommendation() {
            Some(recommendation) => println!("{}", recommendation.human_summary()),
            None => println!("{}", analysis.summary().human_empty_summary()),
        }
        for warning in &warnings {
            print_warning(warning);
        }
    }
    Ok(())
}

fn synchronize(repository: &Repository) -> Result<LocalReplica, CliError> {
    let client = github_client()?;
    let replica = replica_sync::fetch(&client, repository)?;
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
    let snapshot = snapshot_summary(replica);
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

fn snapshot_summary(replica: &LocalReplica) -> SnapshotSummary<'_> {
    SnapshotSummary {
        schema_version: &replica.schema_version,
        synced_at: &replica.synced_at,
        input_hash: &replica.input_hash,
        issue_count: replica.issues.len(),
        comment_count: replica
            .issues
            .iter()
            .map(|issue| issue.comments.len())
            .sum(),
        dependency_count: replica.dependencies.len(),
    }
}

fn ready(repository: &Repository, assignee: Option<&str>, json: bool) -> Result<(), CliError> {
    let (replica, source) = refresh_or_local(repository)?;
    let outbox = OutboxStore::discover(repository)?.load(repository)?;
    let working = WorkingGraph::project(&replica, &outbox)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let analysis = analyze_ready(&replica, scope);
    let warnings = analysis_warnings(&working, source);
    let issues: Vec<_> = analysis
        .executable
        .iter()
        .map(|issue| ReadyIssue {
            provenance: working.provenance_for_issue(issue.number),
            number: issue.number,
            url: &issue.url,
            title: &issue.title,
            ready: true,
            available: issue.assignees.is_empty(),
            priority: working.priority(issue),
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
        replica_snapshot_hash: &replica.input_hash,
        input_hash: working.input_hash(),
        pending: working.is_pending(),
        pending_operation_ids: working.operation_ids(),
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

fn analysis_warnings(working: &WorkingGraph<'_>, source: ReplicaSource) -> Vec<ReadyWarning> {
    let replica = working.replica();
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
        let priority = working.priority(issue);
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
struct PriorityUpdateOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    status: PriorityUpdateStatus,
    pending: bool,
    repository: &'a str,
    issue: PriorityIssueOutput<'a>,
    previous_priority: PriorityState,
    resulting_priority: PriorityState,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<PendingOperationOutput<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    working_graph: Option<WorkingGraphSummary<'a>>,
    snapshot: SnapshotSummary<'a>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum PriorityUpdateStatus {
    Synchronized,
    Pending,
}

#[derive(Serialize)]
struct PendingOperationOutput<'a> {
    id: &'a str,
    kind: &'static str,
    base: &'a LogicalPriority,
    desired: &'a LogicalPriority,
}

impl<'a> From<&'a PendingMutation> for PendingOperationOutput<'a> {
    fn from(operation: &'a PendingMutation) -> Self {
        Self {
            id: operation.id(),
            kind: "priority_update",
            base: operation.base(),
            desired: operation.desired(),
        }
    }
}

#[derive(Serialize)]
struct WorkingGraphSummary<'a> {
    input_hash: &'a str,
}

#[derive(Serialize)]
struct PriorityIssueOutput<'a> {
    key: &'a str,
    number: u64,
    url: &'a str,
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
    replica_snapshot_hash: &'a str,
    input_hash: &'a str,
    pending: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pending_operation_ids: Vec<String>,
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
    #[serde(flatten)]
    provenance: PendingProvenance,
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
    #[error(transparent)]
    IssueReference(#[from] IssueReferenceError),
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
    ReplicaSync(#[from] ReplicaSyncError),
    #[error(transparent)]
    PriorityUpdate(#[from] PriorityUpdateError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    #[error(transparent)]
    WorkingGraph(#[from] WorkingGraphError),
    #[error(
        "online Priority update was unavailable ({online}); the Pending mutation could not be queued: {queue}"
    )]
    OnlinePriorityUpdateAndQueueFailed {
        online: String,
        queue: PendingPriorityUpdateError,
    },
    #[error("could not encode command JSON output: {0}")]
    EncodeOutput(serde_json::Error),
    #[error("this implementation supports only next/v1 horizon 1, not horizon {0}")]
    UnsupportedNextHorizon(u8),
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
