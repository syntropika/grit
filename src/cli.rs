use std::{env, path::PathBuf, time::Instant};

use clap::{Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::{
    auth::{AuthError, AuthToken},
    github::{
        CreateLabelRequest, DependencyChange, DependencyIntent, GitHubClient, GitHubError,
        LabelCreation,
    },
    graph::{GraphError, PublicGraphOptions, confirm_public_repository, publish_site},
    model::{LocalReplica, ReplicaError},
    operational::{ExecutionScope, PreparedRepository, analyze_ready},
    outbox::{OutboxError, OutboxStore, PendingMutation},
    plan::{DependencyLayers, PlanIssue},
    priority::{
        DeclaredPriority, LogicalPriority, PrioritySelection, PriorityState,
        missing_canonical_labels, present_canonical_labels,
    },
    priority_update::{self, PendingPriorityUpdateError, PriorityUpdateError},
    ranking::{self, NextAnalysis, PlanDecision},
    replica_sync::{self, ReplicaSyncError},
    repository::{IssueReference, IssueReferenceError, Repository, RepositoryError},
    store::{ReplicaStore, StoreError},
    triage::{self, TriageReport},
    working_graph::{PendingProvenance, WorkingGraph, WorkingGraphError},
};

const SYNC_SCHEMA_VERSION: &str = "grit.sync/v1";
const READY_SCHEMA_VERSION: &str = "grit.ready/v1";
const GRAPH_SCHEMA_VERSION: &str = "grit.graph/v1";
const DEPENDENCY_MUTATION_SCHEMA_VERSION: &str = "grit.dependency-mutation/v1";
const INIT_SCHEMA_VERSION: &str = "grit.init/v1";
const PLAN_SCHEMA_VERSION: &str = "grit.plan/v1";
const TRIAGE_SCHEMA_VERSION: &str = "grit.triage/v1";
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
        /// Number of completions to evaluate, from one through the default three.
        #[arg(long, default_value_t = ranking::DEFAULT_HORIZON)]
        horizon: u8,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
        /// Report local ranking phase timings; Synchronization is excluded.
        #[arg(long)]
        profile: bool,
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
    /// Surface actionable operational graph problems.
    Triage {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Evaluate execution-scope membership for this GitHub login.
        #[arg(long)]
        assignee: Option<String>,
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
    /// Generate a deterministic static Issue graph site.
    Graph {
        /// Repository in OWNER/REPO form.
        #[arg(long)]
        repo: String,
        /// Target directory for the complete static site.
        #[arg(long)]
        output: PathBuf,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
        /// Generate a fail-closed artifact safe for deliberate public publication.
        #[arg(long)]
        public: bool,
        /// Publish labels in this explicitly allowed category prefix. Repeatable.
        #[arg(long, requires = "public")]
        public_label_prefix: Vec<String>,
        /// Publish GitHub assignee logins in the public artifact.
        #[arg(long, requires = "public")]
        public_include_assignees: bool,
    },
    /// Make one Issue blocked by another native GitHub Issue.
    Block {
        /// Issue to block in OWNER/REPO#NUMBER form.
        issue: String,
        /// Blocking Issue in OWNER/REPO#NUMBER form.
        #[arg(long)]
        by: String,
        /// Emit versioned machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Remove a native blocked-by relationship between two Issues.
    Unblock {
        /// Issue that is currently blocked in OWNER/REPO#NUMBER form.
        issue: String,
        /// Blocking Issue in OWNER/REPO#NUMBER form.
        #[arg(long)]
        by: String,
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
            json,
            public,
            public_label_prefix,
            public_include_assignees,
        } => graph(
            &Repository::parse(&repo)?,
            &output,
            json,
            public,
            PublicGraphOptions {
                label_prefixes: public_label_prefix,
                include_assignees: public_include_assignees,
            },
        ),
        Command::Next {
            repo,
            assignee,
            horizon,
            json,
            profile,
        } => next(
            &Repository::parse(&repo)?,
            assignee.as_deref(),
            horizon,
            json,
            profile,
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
        Command::Triage {
            repo,
            assignee,
            json,
        } => triage_command(&Repository::parse(&repo)?, assignee.as_deref(), json),
        Command::Update {
            issue,
            priority,
            json,
        } => update_priority(&issue, priority, json),
        Command::Block { issue, by, json } => {
            mutate_dependency(&issue, &by, DependencyIntent::Block, json)
        }
        Command::Unblock { issue, by, json } => {
            mutate_dependency(&issue, &by, DependencyIntent::Unblock, json)
        }
        Command::Init { repo, json } => initialize(&Repository::parse(&repo)?, json),
        Command::Sync { repo, json } => sync(&Repository::parse(&repo)?, json),
        Command::Ready {
            repo,
            assignee,
            json,
        } => ready(&Repository::parse(&repo)?, assignee.as_deref(), json),
    }
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
    let outbox = OutboxStore::discover(repository)?.load(repository)?;
    let working = WorkingGraph::project(&replica, &outbox)?;
    let prepared = PreparedRepository::prepare(&working);
    let store = ReplicaStore::discover(repository)?;
    let mut cache = ranking::RankingCache::at(store.repository_directory());
    let decision = ranking::analyze_prepared(&prepared, scope, horizon, &mut cache)
        .analysis
        .into_plan_decision();
    let structural = crate::plan::analyze(&prepared, scope);
    let parallel_now = structural.parallel_now;
    let dependency_layers = structural.dependency_layers;
    let warnings = analysis_warnings(&working, source);

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
        match decision.recommendation() {
            Some(recommendation) => println!("{}", recommendation.human_summary()),
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

fn graph(
    repository: &Repository,
    output: &std::path::Path,
    json: bool,
    public: bool,
    public_options: PublicGraphOptions,
) -> Result<(), CliError> {
    let (replica, source, site) = if public {
        let client = github_client()?;
        let metadata = client.fetch_repository_metadata(repository)?;
        let confirmed = confirm_public_repository(repository.full_name(), metadata)?;
        let (replica, source) = refresh_or_local_with_client(repository, &client)?;
        let site =
            crate::graph::publish_public_site(&replica, &confirmed, &public_options, output)?;
        (replica, source, site)
    } else {
        let (replica, source) = refresh_or_local(repository)?;
        let site = publish_site(&replica, output)?;
        (replica, source, site)
    };
    let output_path = output.display().to_string();
    let result = GraphOutput {
        schema_version: GRAPH_SCHEMA_VERSION,
        command: "graph",
        repository: &replica.repository,
        source,
        synced_at: &replica.synced_at,
        input_hash: &site.input_hash,
        output: &output_path,
        artifact: GraphArtifactSummary {
            schema_version: site.schema_version,
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

fn triage_command(
    repository: &Repository,
    assignee: Option<&str>,
    json: bool,
) -> Result<(), CliError> {
    let (replica, source) = refresh_or_local(repository)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let report = triage::analyze(&replica, scope);
    let warnings: Vec<_> = source.warning().into_iter().collect();
    if json {
        let output = TriageOutput {
            schema_version: TRIAGE_SCHEMA_VERSION,
            command: "triage",
            repository: &replica.repository,
            source,
            synced_at: &replica.synced_at,
            input_hash: &replica.input_hash,
            execution_scope: execution_scope_output(assignee),
            report,
            warnings,
        };
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!(
            "Triage diagnostics in {} (scope {}, synced_at {}):",
            replica.repository,
            execution_scope_name(assignee),
            replica.synced_at
        );
        let lines = report.human_lines();
        if lines.is_empty() {
            println!("No actionable graph problems");
        } else {
            for line in lines {
                println!("{line}");
            }
        }
        for warning in warnings {
            eprintln!("warning: {}", warning.message);
        }
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
    profile: bool,
) -> Result<(), CliError> {
    if !(ranking::MIN_HORIZON..=ranking::MAX_HORIZON).contains(&horizon) {
        return Err(CliError::UnsupportedNextHorizon(horizon));
    }
    let (replica, source) = refresh_or_local(repository)?;
    let outbox = OutboxStore::discover(repository)?.load(repository)?;
    let working = WorkingGraph::project(&replica, &outbox)?;
    let scope = assignee
        .map(ExecutionScope::Assignee)
        .unwrap_or(ExecutionScope::Available);
    let store = ReplicaStore::discover(repository)?;
    let mut cache = ranking::RankingCache::at(store.repository_directory());
    let run = ranking::analyze_profiled(&working, scope, horizon, &mut cache);
    let analysis = run.analysis;
    let analysis_serialization = (profile && json).then(|| {
        let serialization_started = Instant::now();
        let _ = serde_json::to_vec(&analysis).expect("Next analysis is serializable");
        serialization_started.elapsed()
    });
    let performance =
        profile.then(|| PerformanceOutput::from_profile(run.profile, analysis_serialization));
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
            performance,
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
        if let Some(warning) = analysis.truncation_warning() {
            eprintln!("warning: {warning}");
        }
        for warning in &warnings {
            print_warning(warning);
        }
        if let Some(performance) = performance {
            eprintln!("{}", performance.human_summary());
        }
    }
    Ok(())
}

fn synchronize(repository: &Repository) -> Result<LocalReplica, CliError> {
    let client = github_client()?;
    synchronize_with_client(repository, &client)
}

fn synchronize_with_client(
    repository: &Repository,
    client: &GitHubClient,
) -> Result<LocalReplica, CliError> {
    let store = ReplicaStore::discover(repository)?;
    let previous = match store.load(repository) {
        Ok(replica) => Some(replica),
        Err(StoreError::MissingReplica | StoreError::Decode(_) | StoreError::InvalidReplica(_)) => {
            None
        }
        Err(error) => return Err(error.into()),
    };
    let replica = replica_sync::refresh(client, repository, previous.as_ref())?;
    store.publish(&replica)?;
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

fn mutate_dependency(
    blocked: &str,
    blocker: &str,
    intent: DependencyIntent,
    json: bool,
) -> Result<(), CliError> {
    let blocked = IssueReference::parse(blocked)?;
    let blocker = IssueReference::parse(blocker)?;
    let client = github_client()?;
    let result = client.mutate_dependency(&blocked, &blocker, intent)?;

    let replica = replica_sync::fetch(&client, blocked.repository()).map_err(|source| {
        CliError::MutationSynchronization {
            source: Box::new(source.into()),
        }
    })?;
    if replica.has_dependency(&blocked, &blocker) != intent.desired_present() {
        return Err(CliError::MutationReadbackMismatch {
            blocked: blocked.stable_key(),
            blocker: blocker.stable_key(),
            expected: dependency_expected_relationship(intent),
        });
    }
    ReplicaStore::discover(blocked.repository())
        .map_err(|source| CliError::MutationPublication { source })?
        .publish(&replica)
        .map_err(|source| CliError::MutationPublication { source })?;

    let snapshot = snapshot_summary(&replica);
    let output = DependencyMutationOutput {
        schema_version: DEPENDENCY_MUTATION_SCHEMA_VERSION,
        command: dependency_command_name(intent),
        repository: blocked.repository().full_name(),
        result,
        edge: DependencyEdgeOutput {
            blocked: blocked.stable_key(),
            blocker: blocker.stable_key(),
            kind: "blocked_by",
        },
        snapshot,
    };
    if json {
        serde_json::to_writer(std::io::stdout().lock(), &output).map_err(CliError::EncodeOutput)?;
        println!();
    } else {
        println!("{}", dependency_human_message(&output.edge, result));
    }
    Ok(())
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
    let comment_count = replica
        .issues
        .iter()
        .map(|issue| issue.comments.len())
        .sum();
    SnapshotSummary {
        schema_version: &replica.schema_version,
        synced_at: &replica.synced_at,
        input_hash: &replica.input_hash,
        issue_count: replica.issues.len(),
        comment_count,
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
    let refresh = github_client().and_then(|client| synchronize_with_client(repository, &client));
    refresh_or_local_after(repository, refresh)
}

fn refresh_or_local_with_client(
    repository: &Repository,
    client: &GitHubClient,
) -> Result<(LocalReplica, ReplicaSource), CliError> {
    let refresh = synchronize_with_client(repository, client);
    refresh_or_local_after(repository, refresh)
}

fn refresh_or_local_after(
    repository: &Repository,
    refresh: Result<LocalReplica, CliError>,
) -> Result<(LocalReplica, ReplicaSource), CliError> {
    match refresh {
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

fn execution_scope_name(assignee: Option<&str>) -> String {
    assignee
        .map(|assignee| format!("assignee:{assignee}"))
        .unwrap_or_else(|| "available".to_owned())
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
struct DependencyMutationOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    repository: &'a str,
    result: DependencyChange,
    edge: DependencyEdgeOutput,
    snapshot: SnapshotSummary<'a>,
}

#[derive(Serialize)]
struct DependencyEdgeOutput {
    blocked: String,
    blocker: String,
    kind: &'static str,
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
struct TriageOutput<'a> {
    schema_version: &'static str,
    command: &'static str,
    repository: &'a str,
    source: ReplicaSource,
    synced_at: &'a str,
    input_hash: &'a str,
    execution_scope: ExecutionScopeOutput<'a>,
    #[serde(flatten)]
    report: TriageReport,
    warnings: Vec<ReadyWarning>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    performance: Option<PerformanceOutput>,
}

#[derive(Serialize)]
struct PerformanceOutput {
    unit: &'static str,
    graph_preparation: u128,
    scc_detection: u128,
    readiness: u128,
    cache_lookup: u128,
    pagerank: u128,
    search: u128,
    output_assembly: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    analysis_serialization: Option<u128>,
    cache_publication: u128,
    total_before_serialization: u128,
    cache_hit: bool,
    cache_published: bool,
    synchronization_included: bool,
}

impl PerformanceOutput {
    fn from_profile(
        profile: ranking::AnalysisProfile,
        analysis_serialization: Option<std::time::Duration>,
    ) -> Self {
        Self {
            unit: "microseconds",
            graph_preparation: profile.graph_preparation.as_micros(),
            scc_detection: profile.scc_detection.as_micros(),
            readiness: profile.readiness.as_micros(),
            cache_lookup: profile.cache_lookup.as_micros(),
            pagerank: profile.pagerank.as_micros(),
            search: profile.search.as_micros(),
            output_assembly: profile.output_assembly.as_micros(),
            analysis_serialization: analysis_serialization.map(|duration| duration.as_micros()),
            cache_publication: profile.cache_publication.as_micros(),
            total_before_serialization: profile.total.as_micros(),
            cache_hit: profile.cache_hit,
            cache_published: profile.cache_published,
            synchronization_included: false,
        }
    }

    fn human_summary(&self) -> String {
        format!(
            "ranking profile (microseconds, Synchronization excluded): graph={} scc={} readiness={} cache={} pagerank={} search={} output={} cache_hit={}",
            self.graph_preparation,
            self.scc_detection,
            self.readiness,
            self.cache_lookup,
            self.pagerank,
            self.search,
            self.output_assembly,
            self.cache_hit,
        )
    }
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
    parallel_now: Vec<PlanIssue<'a>>,
    dependency_layers: DependencyLayers<'a>,
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

fn dependency_command_name(intent: DependencyIntent) -> &'static str {
    match intent {
        DependencyIntent::Block => "block",
        DependencyIntent::Unblock => "unblock",
    }
}

fn dependency_expected_relationship(intent: DependencyIntent) -> &'static str {
    if intent.desired_present() {
        "present"
    } else {
        "absent"
    }
}

fn dependency_human_message(edge: &DependencyEdgeOutput, result: DependencyChange) -> String {
    match result {
        DependencyChange::Created => {
            format!("{} is now blocked by {}", edge.blocked, edge.blocker)
        }
        DependencyChange::AlreadyPresent => {
            format!("{} was already blocked by {}", edge.blocked, edge.blocker)
        }
        DependencyChange::Removed => {
            format!("{} is no longer blocked by {}", edge.blocked, edge.blocker)
        }
        DependencyChange::AlreadyAbsent => {
            format!("{} was not blocked by {}", edge.blocked, edge.blocker)
        }
    }
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
    #[error(
        "GitHub dependency operation completed, but synchronized readback failed; Local replica was not changed: {source}"
    )]
    MutationSynchronization {
        #[source]
        source: Box<CliError>,
    },
    #[error(
        "GitHub dependency operation completed and readback was verified, but Local replica publication failed: {source}"
    )]
    MutationPublication {
        #[source]
        source: StoreError,
    },
    #[error(
        "GitHub dependency operation completed, but synchronized readback did not show {blocked} blocked by {blocker} as {expected}; Local replica was not changed"
    )]
    MutationReadbackMismatch {
        blocked: String,
        blocker: String,
        expected: &'static str,
    },
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
