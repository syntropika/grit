use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use thiserror::Error;

use crate::{
    github::GitHubClient,
    model::{Issue, LocalReplica},
    outbox::{MutationState, OutboxError, OutboxStore, PendingMutation, PriorityWrite},
    priority::{DeclaredPriority, LogicalPriority, PrioritySelection, PriorityState},
    replica_sync::{self, ReplicaSyncError},
    repository::Repository,
    store::{ReplicaStore, StoreError},
};

pub(crate) const OUTPUT_SCHEMA_VERSION: &str = "grit.reconcile/v1";

#[derive(Clone, Copy)]
pub(crate) enum ResolutionChoice {
    Remote,
    Local,
    Replacement(PrioritySelection),
}

pub(crate) struct ResolutionResult {
    pub(crate) operation_id: String,
    pub(crate) choice: &'static str,
    pub(crate) reconciliation: ReconciliationResult,
}

#[derive(Serialize)]
pub(crate) struct ReconciliationResult {
    pub(crate) repository: String,
    pub(crate) operations: Vec<OperationResult>,
    pub(crate) summary: ReconciliationSummary,
    #[serde(skip)]
    pub(crate) replica: LocalReplica,
}

#[derive(Clone, Serialize)]
pub(crate) struct OperationResult {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) issue_number: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) depends_on: Vec<String>,
    pub(crate) classification: Classification,
    pub(crate) outcome: Outcome,
    pub(crate) base: LogicalPriority,
    pub(crate) local: LogicalPriority,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remote: Option<LogicalPriority>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) blocked_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Classification {
    Applicable,
    AlreadySatisfied,
    Conflicting,
    TransitivelyBlocked,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Applied,
    AlreadySatisfied,
    Conflicting,
    TransitivelyBlocked,
    Checkpointed,
    Failed,
    ResolvedRemote,
}

#[derive(Default, Serialize)]
pub(crate) struct ReconciliationSummary {
    pub(crate) applicable: usize,
    pub(crate) already_satisfied: usize,
    pub(crate) conflicting: usize,
    pub(crate) transitively_blocked: usize,
    pub(crate) applied: usize,
    pub(crate) checkpointed: usize,
    pub(crate) failed: usize,
    pub(crate) remaining: usize,
}

impl ReconciliationSummary {
    fn from_operations(operations: &[OperationResult], remaining: usize) -> Self {
        let mut summary = Self {
            remaining,
            ..Self::default()
        };
        for operation in operations {
            match operation.classification {
                Classification::Applicable => summary.applicable += 1,
                Classification::AlreadySatisfied => summary.already_satisfied += 1,
                Classification::Conflicting => summary.conflicting += 1,
                Classification::TransitivelyBlocked => summary.transitively_blocked += 1,
            }
            match operation.outcome {
                Outcome::Applied => summary.applied += 1,
                Outcome::Checkpointed => summary.checkpointed += 1,
                Outcome::Failed => summary.failed += 1,
                Outcome::AlreadySatisfied
                | Outcome::Conflicting
                | Outcome::TransitivelyBlocked
                | Outcome::ResolvedRemote => {}
            }
        }
        summary
    }
}

#[derive(Clone)]
struct RemotePriority {
    logical: LogicalPriority,
    canonical_labels: Vec<String>,
}

pub(crate) fn reconcile(
    client: &GitHubClient,
    repository: &Repository,
) -> Result<ReconciliationResult, ReconciliationError> {
    let outbox_store = OutboxStore::discover(repository)?;
    let mut transaction = outbox_store.begin_transaction(repository)?;
    let preflight = replica_sync::fetch(client, repository)?;
    let pass = ReconciliationPass {
        client,
        repository,
        transaction: &mut transaction,
        remote: remote_priorities(&preflight),
        results: Vec::new(),
        requires_final_refresh: false,
    }
    .run()?;
    let mut results = pass.operations;

    let final_replica = if pass.requires_final_refresh {
        replica_sync::fetch(client, repository)
            .map_err(ReconciliationError::FinalSynchronization)?
    } else {
        preflight
    };
    ReplicaStore::discover(repository)?
        .publish(&final_replica)
        .map_err(ReconciliationError::FinalPublication)?;

    retire_verified_operations(repository, &mut transaction, &final_replica, &mut results)?;
    let remaining = transaction.operations().len();
    let summary = ReconciliationSummary::from_operations(&results, remaining);

    Ok(ReconciliationResult {
        repository: repository.full_name().to_owned(),
        operations: results,
        summary,
        replica: final_replica,
    })
}

struct ReconciliationPass<'client, 'transaction, 'store> {
    client: &'client GitHubClient,
    repository: &'client Repository,
    transaction: &'transaction mut crate::outbox::OutboxTransaction<'store>,
    remote: BTreeMap<u64, RemotePriority>,
    results: Vec<OperationResult>,
    requires_final_refresh: bool,
}

impl ReconciliationPass<'_, '_, '_> {
    fn run(mut self) -> Result<PassResult, ReconciliationError> {
        let operation_ids: Vec<_> = self
            .transaction
            .operations()
            .iter()
            .map(|operation| operation.id().to_owned())
            .collect();
        self.results.reserve(operation_ids.len());
        for operation_id in operation_ids {
            self.reconcile_operation(&operation_id)?;
        }
        Ok(PassResult {
            operations: self.results,
            requires_final_refresh: self.requires_final_refresh,
        })
    }

    fn reconcile_operation(&mut self, operation_id: &str) -> Result<(), ReconciliationError> {
        let operation = self
            .transaction
            .operation(operation_id)
            .expect("operation ID came from this transaction")
            .clone();
        let blocked_by = self.blocking_dependencies(&operation);
        if !blocked_by.is_empty() {
            self.stage_state(
                operation_id,
                MutationState::TransitivelyBlocked {
                    blocked_by: blocked_by.clone(),
                },
            )?;
            self.record(
                &operation,
                Classification::TransitivelyBlocked,
                Outcome::TransitivelyBlocked,
                self.observed_priority(operation.issue_number()),
                blocked_by,
                None,
            );
            return Ok(());
        }

        let Some(remote) = self.remote.get(&operation.issue_number()).cloned() else {
            return self.record_missing_issue(&operation);
        };
        let state = self
            .transaction
            .operation(operation_id)
            .expect("known operation")
            .state()
            .clone();
        self.reconcile_state(&operation, state, remote)
    }

    fn reconcile_state(
        &mut self,
        operation: &PendingMutation,
        state: MutationState,
        mut remote: RemotePriority,
    ) -> Result<(), ReconciliationError> {
        match state {
            MutationState::Conflicting { .. } => self.classify_pending(operation, &mut remote),
            MutationState::Applied => {
                self.record_terminal(operation, Classification::Applicable, remote.logical)
            }
            MutationState::AlreadySatisfied => {
                self.record_terminal(operation, Classification::AlreadySatisfied, remote.logical)
            }
            MutationState::ResolvedRemote { .. } => {
                self.record(
                    operation,
                    Classification::AlreadySatisfied,
                    Outcome::ResolvedRemote,
                    Some(remote.logical),
                    Vec::new(),
                    None,
                );
                Ok(())
            }
            MutationState::Applying {
                expected_labels,
                remaining_writes,
                ..
            } => {
                if remote.canonical_labels != expected_labels {
                    if remote.logical == *operation.desired() {
                        self.checkpoint_state(operation.id(), MutationState::Applied)?;
                        self.record(
                            operation,
                            Classification::Applicable,
                            Outcome::Applied,
                            Some(remote.logical),
                            Vec::new(),
                            None,
                        );
                        Ok(())
                    } else {
                        self.record_conflict(operation, remote.logical)
                    }
                } else {
                    self.execute_plan(operation, expected_labels, remaining_writes, &mut remote)
                }
            }
            MutationState::Pending
            | MutationState::Failed { .. }
            | MutationState::TransitivelyBlocked { .. } => {
                self.classify_pending(operation, &mut remote)
            }
        }
    }

    fn classify_pending(
        &mut self,
        operation: &PendingMutation,
        remote: &mut RemotePriority,
    ) -> Result<(), ReconciliationError> {
        match classify_priority(operation.base(), operation.desired(), &remote.logical) {
            PendingClassification::AlreadySatisfied => {
                self.stage_state(operation.id(), MutationState::AlreadySatisfied)?;
                self.record(
                    operation,
                    Classification::AlreadySatisfied,
                    Outcome::AlreadySatisfied,
                    Some(remote.logical.clone()),
                    Vec::new(),
                    None,
                );
                Ok(())
            }
            PendingClassification::Conflicting => {
                self.record_conflict(operation, remote.logical.clone())
            }
            PendingClassification::Applicable => {
                let writes =
                    PriorityWrite::canonical_plan(&remote.canonical_labels, operation.desired())
                        .expect("remote Priority labels and desired outbox value are valid");
                self.checkpoint_state(
                    operation.id(),
                    MutationState::Applying {
                        expected_labels: remote.canonical_labels.clone(),
                        remaining_writes: writes.clone(),
                        last_error: None,
                    },
                )?;
                self.execute_plan(operation, remote.canonical_labels.clone(), writes, remote)
            }
        }
    }

    fn execute_plan(
        &mut self,
        operation: &PendingMutation,
        expected_labels: Vec<String>,
        remaining_writes: Vec<PriorityWrite>,
        remote: &mut RemotePriority,
    ) -> Result<(), ReconciliationError> {
        let (outcome, error) =
            self.execute_writes(operation, expected_labels, remaining_writes, remote)?;
        self.remote.insert(operation.issue_number(), remote.clone());
        self.record(
            operation,
            Classification::Applicable,
            outcome,
            Some(remote.logical.clone()),
            Vec::new(),
            error,
        );
        Ok(())
    }

    fn execute_writes(
        &mut self,
        operation: &PendingMutation,
        mut expected_labels: Vec<String>,
        mut remaining_writes: Vec<PriorityWrite>,
        remote: &mut RemotePriority,
    ) -> Result<(Outcome, Option<String>), ReconciliationError> {
        while let Some(write) = remaining_writes.first().cloned() {
            self.requires_final_refresh = true;
            let result = match &write {
                PriorityWrite::Add { label } => {
                    self.client
                        .add_issue_label(self.repository, operation.issue_number(), label)
                }
                PriorityWrite::Remove { label } => {
                    self.client
                        .remove_issue_label(self.repository, operation.issue_number(), label)
                }
            };
            if let Err(error) = result {
                let message = error.to_string();
                self.checkpoint_state(
                    operation.id(),
                    MutationState::Applying {
                        expected_labels,
                        remaining_writes,
                        last_error: Some(message.clone()),
                    },
                )?;
                return Ok((Outcome::Failed, Some(message)));
            }
            write.apply_to(&mut expected_labels);
            remaining_writes.remove(0);
            let state = if remaining_writes.is_empty() {
                MutationState::Applied
            } else {
                MutationState::Applying {
                    expected_labels: expected_labels.clone(),
                    remaining_writes: remaining_writes.clone(),
                    last_error: None,
                }
            };
            self.checkpoint_state(operation.id(), state)?;
        }
        remote.canonical_labels = expected_labels;
        remote.logical = LogicalPriority::from_canonical_labels(&remote.canonical_labels)
            .expect("write plans retain canonical Priority labels");
        Ok((Outcome::Applied, None))
    }

    fn record_conflict(
        &mut self,
        operation: &PendingMutation,
        observed: LogicalPriority,
    ) -> Result<(), ReconciliationError> {
        self.stage_state(
            operation.id(),
            MutationState::Conflicting {
                remote: observed.clone(),
            },
        )?;
        self.record(
            operation,
            Classification::Conflicting,
            Outcome::Conflicting,
            Some(observed),
            Vec::new(),
            None,
        );
        Ok(())
    }

    fn record_terminal(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        observed: LogicalPriority,
    ) -> Result<(), ReconciliationError> {
        self.record(
            operation,
            classification,
            Outcome::Checkpointed,
            Some(observed),
            Vec::new(),
            None,
        );
        Ok(())
    }

    fn record_missing_issue(
        &mut self,
        operation: &PendingMutation,
    ) -> Result<(), ReconciliationError> {
        let error = format!("Issue #{} is absent from GitHub", operation.issue_number());
        self.stage_state(
            operation.id(),
            MutationState::Failed {
                error: error.clone(),
            },
        )?;
        self.record(
            operation,
            Classification::Applicable,
            Outcome::Failed,
            None,
            Vec::new(),
            Some(error),
        );
        Ok(())
    }

    fn checkpoint_state(
        &mut self,
        operation_id: &str,
        state: MutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .checkpoint_state(self.repository, operation_id, state)?;
        Ok(())
    }

    fn stage_state(
        &mut self,
        operation_id: &str,
        state: MutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction.stage_state(operation_id, state)?;
        Ok(())
    }

    fn blocking_dependencies(&self, operation: &PendingMutation) -> Vec<String> {
        operation
            .depends_on()
            .iter()
            .filter(|dependency| {
                self.transaction
                    .operation(dependency)
                    .is_some_and(|dependency| !dependency.state().permits_dependents())
            })
            .cloned()
            .collect()
    }

    fn observed_priority(&self, issue_number: u64) -> Option<LogicalPriority> {
        self.remote
            .get(&issue_number)
            .map(|priority| priority.logical.clone())
    }

    fn record(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        outcome: Outcome,
        remote: Option<LogicalPriority>,
        blocked_by: Vec<String>,
        error: Option<String>,
    ) {
        self.results.push(result_for(
            operation,
            classification,
            outcome,
            remote,
            blocked_by,
            error,
        ));
    }
}

struct PassResult {
    operations: Vec<OperationResult>,
    requires_final_refresh: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingClassification {
    Applicable,
    AlreadySatisfied,
    Conflicting,
}

fn classify_priority(
    base: &LogicalPriority,
    desired: &LogicalPriority,
    remote: &LogicalPriority,
) -> PendingClassification {
    if remote == desired {
        PendingClassification::AlreadySatisfied
    } else if remote == base {
        PendingClassification::Applicable
    } else {
        PendingClassification::Conflicting
    }
}

fn retire_verified_operations(
    repository: &Repository,
    transaction: &mut crate::outbox::OutboxTransaction<'_>,
    final_replica: &LocalReplica,
    results: &mut [OperationResult],
) -> Result<(), ReconciliationError> {
    let final_priorities = remote_priorities(final_replica);
    let terminal_ids: Vec<_> = transaction
        .operations()
        .iter()
        .filter(|operation| operation.state().is_successfully_terminal())
        .map(|operation| operation.id().to_owned())
        .collect();
    let terminal_set: BTreeSet<_> = terminal_ids.iter().cloned().collect();
    let superseded = superseded_terminal_ids(transaction.operations(), &terminal_set);
    let mut retired = BTreeSet::new();
    let mut state_updates = BTreeMap::new();
    for operation_id in terminal_ids.into_iter().rev() {
        let operation = transaction
            .operation(&operation_id)
            .expect("known terminal operation")
            .clone();
        if superseded.contains(&operation_id)
            || matches!(operation.state(), MutationState::ResolvedRemote { .. })
        {
            retired.insert(operation_id);
            continue;
        }
        let observed = final_priorities
            .get(&operation.issue_number())
            .map(|priority| priority.logical.clone());
        if observed.as_ref() == Some(operation.desired()) {
            retired.insert(operation_id);
        } else {
            convert_to_late_conflict(results, &operation_id, observed, &mut state_updates);
        }
    }
    transaction.finalize(repository, &retired, state_updates)?;
    Ok(())
}

fn superseded_terminal_ids(
    operations: &[PendingMutation],
    terminal: &BTreeSet<String>,
) -> BTreeSet<String> {
    let issue_by_id: BTreeMap<_, _> = operations
        .iter()
        .map(|operation| (operation.id(), operation.issue_number()))
        .collect();
    operations
        .iter()
        .filter(|operation| terminal.contains(operation.id()))
        .flat_map(|operation| {
            operation.depends_on().iter().filter(|dependency| {
                issue_by_id
                    .get(dependency.as_str())
                    .is_some_and(|issue| *issue == operation.issue_number())
            })
        })
        .cloned()
        .collect()
}

fn convert_to_late_conflict(
    results: &mut [OperationResult],
    operation_id: &str,
    observed: Option<LogicalPriority>,
    state_updates: &mut BTreeMap<String, MutationState>,
) {
    let observed = observed.unwrap_or(LogicalPriority::Unspecified);
    state_updates.insert(
        operation_id.to_owned(),
        MutationState::Conflicting {
            remote: observed.clone(),
        },
    );
    if let Some(result) = results.iter_mut().find(|result| result.id == operation_id) {
        result.classification = Classification::Conflicting;
        result.outcome = Outcome::Conflicting;
        result.remote = Some(observed);
        result.error = None;
    }
}

pub(crate) fn resolve(
    client: &GitHubClient,
    repository: &Repository,
    operation_id: &str,
    choice: ResolutionChoice,
) -> Result<ResolutionResult, ReconciliationError> {
    let store = OutboxStore::discover(repository)?;
    let mut transaction = store.begin_transaction(repository)?;
    let operation = transaction
        .operation(operation_id)
        .ok_or_else(|| ReconciliationError::UnknownOperation(operation_id.to_owned()))?;
    let MutationState::Conflicting { remote } = operation.state().clone() else {
        return Err(ReconciliationError::OperationNotConflicting(
            operation_id.to_owned(),
        ));
    };
    let choice_name = match choice {
        ResolutionChoice::Remote => "remote",
        ResolutionChoice::Local => "local",
        ResolutionChoice::Replacement(_) => "priority",
    };
    match choice {
        ResolutionChoice::Remote => {
            transaction.resolve_remote(repository, operation_id, remote)?;
        }
        ResolutionChoice::Local => {
            transaction.rebase_priority(repository, operation_id, remote, None)?;
        }
        ResolutionChoice::Replacement(selection) => transaction.rebase_priority(
            repository,
            operation_id,
            remote,
            Some(LogicalPriority::from_selection(selection)),
        )?,
    }
    drop(transaction);

    Ok(ResolutionResult {
        operation_id: operation_id.to_owned(),
        choice: choice_name,
        reconciliation: reconcile(client, repository)?,
    })
}

fn remote_priorities(replica: &LocalReplica) -> BTreeMap<u64, RemotePriority> {
    replica
        .issues
        .iter()
        .map(|issue| (issue.number, remote_priority(issue)))
        .collect()
}

fn remote_priority(issue: &Issue) -> RemotePriority {
    let mut canonical_labels: Vec<_> = issue
        .labels
        .iter()
        .filter_map(|label| {
            DeclaredPriority::parse(&label.name)
                .map(|priority| priority.canonical_label().to_owned())
        })
        .collect();
    canonical_labels.sort();
    canonical_labels.dedup();
    RemotePriority {
        logical: LogicalPriority::from_state(&PriorityState::from_issue_labels(&issue.labels)),
        canonical_labels,
    }
}

fn result_for(
    operation: &PendingMutation,
    classification: Classification,
    outcome: Outcome,
    remote: Option<LogicalPriority>,
    blocked_by: Vec<String>,
    error: Option<String>,
) -> OperationResult {
    OperationResult {
        id: operation.id().to_owned(),
        kind: "priority_update",
        issue_number: operation.issue_number(),
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        base: operation.base().clone(),
        local: operation.desired().clone(),
        remote,
        blocked_by,
        error,
    }
}

#[derive(Debug, Error)]
pub(crate) enum ReconciliationError {
    #[error("could not refresh GitHub before Mutation reconciliation: {0}")]
    Preflight(#[from] ReplicaSyncError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("final Synchronization after Mutation reconciliation failed: {0}")]
    FinalSynchronization(ReplicaSyncError),
    #[error("accepted GitHub state could not be published to the Local replica: {0}")]
    FinalPublication(StoreError),
    #[error("Pending mutation operation {0:?} does not exist")]
    UnknownOperation(String),
    #[error("Pending mutation operation {0:?} is not a Priority conflict")]
    OperationNotConflicting(String),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{PendingClassification, classify_priority, superseded_terminal_ids};
    use crate::{
        outbox::{PendingMutation, PriorityWrite},
        priority::{DeclaredPriority, LogicalPriority},
        repository::Repository,
    };

    #[test]
    fn three_way_priority_classification_uses_the_current_remote_value() {
        let base = declared(DeclaredPriority::P1);
        let desired = declared(DeclaredPriority::P0);

        assert_eq!(
            classify_priority(&base, &desired, &desired),
            PendingClassification::AlreadySatisfied
        );
        assert_eq!(
            classify_priority(&base, &desired, &base),
            PendingClassification::Applicable
        );
        assert_eq!(
            classify_priority(&base, &desired, &declared(DeclaredPriority::P3)),
            PendingClassification::Conflicting
        );
    }

    #[test]
    fn priority_write_plan_adds_desired_before_removing_every_obsolete_value() {
        assert_eq!(
            PriorityWrite::canonical_plan(
                &["priority:p1".to_owned(), "priority:p3".to_owned()],
                &declared(DeclaredPriority::P0),
            )
            .expect("canonical plan"),
            vec![
                PriorityWrite::Add {
                    label: "priority:p0".to_owned()
                },
                PriorityWrite::Remove {
                    label: "priority:p1".to_owned()
                },
                PriorityWrite::Remove {
                    label: "priority:p3".to_owned()
                }
            ]
        );
    }

    #[test]
    fn superseded_terminal_lookup_scales_as_one_precomputed_chain_walk() {
        let repository = Repository::parse("acme/reconcile").expect("repository");
        let mut operations = Vec::with_capacity(5_000);
        let mut dependency = None;
        for _ in 0..5_000 {
            let operation = PendingMutation::priority_update(
                &repository,
                1,
                declared(DeclaredPriority::P1),
                declared(DeclaredPriority::P0),
                dependency.into_iter().collect(),
            );
            dependency = Some(operation.id().to_owned());
            operations.push(operation);
        }
        let terminal: BTreeSet<_> = operations
            .iter()
            .map(|operation| operation.id().to_owned())
            .collect();

        assert_eq!(
            superseded_terminal_ids(&operations, &terminal).len(),
            operations.len() - 1
        );
    }

    fn declared(value: DeclaredPriority) -> LogicalPriority {
        LogicalPriority::Declared { value }
    }
}
