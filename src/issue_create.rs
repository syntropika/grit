use thiserror::Error;

use crate::{
    model::{LocalReplica, StableNodeKey, TemporaryIssueId},
    outbox::{OutboxError, OutboxStore, PendingMutation},
    repository::Repository,
    store::{ReplicaStore, StoreError},
    working_graph::{WorkingGraph, WorkingGraphError},
};

pub(crate) struct PendingIssueCreateResult {
    pub(crate) operation: PendingMutation,
    pub(crate) temporary_id: TemporaryIssueId,
    pub(crate) stable_node_key: StableNodeKey,
    pub(crate) key: String,
    pub(crate) working_input_hash: String,
    pub(crate) replica: LocalReplica,
}

pub(crate) fn queue(
    repository: &Repository,
    title: String,
    body: String,
) -> Result<PendingIssueCreateResult, PendingIssueCreateError> {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return Err(PendingIssueCreateError::EmptyTitle);
    }

    let outbox_store = OutboxStore::discover(repository)?;
    let transaction = outbox_store.begin_transaction(repository)?;
    let replica = ReplicaStore::discover(repository)?.load(repository)?;
    let temporary_id = unique_temporary_id(&replica, transaction.outbox());
    let operation = PendingMutation::issue_create(temporary_id, title, body);
    let next_outbox = transaction.append(repository, operation.clone())?;
    let working = WorkingGraph::project(&replica, &next_outbox)?;

    Ok(PendingIssueCreateResult {
        operation,
        temporary_id,
        stable_node_key: StableNodeKey::Draft(temporary_id),
        key: temporary_id.stable_node_key(repository.full_name()),
        working_input_hash: working.input_hash().to_owned(),
        replica,
    })
}

fn unique_temporary_id(
    replica: &LocalReplica,
    outbox: &crate::outbox::PendingMutationOutbox,
) -> TemporaryIssueId {
    loop {
        let temporary_id = TemporaryIssueId::new();
        let synthetic_number = temporary_id.synthetic_number();
        let replica_collision = replica
            .issues
            .iter()
            .any(|issue| issue.number == synthetic_number);
        let outbox_collision = outbox
            .operations()
            .iter()
            .any(|operation| operation.issue_number() == synthetic_number);
        if !replica_collision && !outbox_collision {
            return temporary_id;
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum PendingIssueCreateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    #[error(transparent)]
    WorkingGraph(#[from] WorkingGraphError),
    #[error("Draft Issue title cannot be empty")]
    EmptyTitle,
}
