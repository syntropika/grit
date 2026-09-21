use thiserror::Error;

use crate::{
    draft_identity::{DraftIdentityError, DraftIdentityStore},
    metadata::{MetadataError, PendingIssueOperand},
    model::{LocalReplica, TemporaryIssueId},
    operation_marker,
    outbox::{OutboxError, OutboxStore, PendingMutation, PendingMutationOutbox},
    repository::PendingIssueReference,
    store::{ReplicaStore, StoreError},
    working_graph::{WorkingGraph, WorkingGraphError},
};

pub(crate) struct PendingCommentCreateResult {
    pub(crate) operation: PendingMutation,
    pub(crate) issue_key: String,
    pub(crate) body: String,
    pub(crate) working_input_hash: String,
    pub(crate) replica: LocalReplica,
}

pub(crate) fn queue(
    reference: &PendingIssueReference,
    body: String,
) -> Result<PendingCommentCreateResult, PendingCommentCreateError> {
    let body = operation_marker::strip(&body);
    if body.trim().is_empty() {
        return Err(PendingCommentCreateError::EmptyBody);
    }
    let repository = reference.repository();
    let store = OutboxStore::discover(repository)?;
    let transaction = store.begin_transaction(repository)?;
    let replica = ReplicaStore::discover(repository)?.load(repository)?;
    let (number, create_dependency) = resolve_reference(reference, transaction.outbox(), &replica)?;
    WorkingGraph::validate_references(&replica, transaction.outbox())?;
    let issue = PendingIssueOperand::new(number, reference.temporary_id())?;
    let operation = PendingMutation::comment_create(
        repository,
        issue,
        body.clone(),
        create_dependency.into_iter().collect(),
    );
    let outbox = transaction.append(repository, operation.clone())?;
    let working = WorkingGraph::project(&replica, &outbox)?;
    Ok(PendingCommentCreateResult {
        operation,
        issue_key: reference.stable_key(),
        body,
        working_input_hash: working.input_hash().to_owned(),
        replica,
    })
}

fn resolve_reference(
    reference: &PendingIssueReference,
    outbox: &PendingMutationOutbox,
    replica: &LocalReplica,
) -> Result<(u64, Option<String>), PendingCommentCreateError> {
    let Some(temporary_id) = reference.temporary_id() else {
        require_issue(replica, reference.local_number(), &reference.stable_key())?;
        return Ok((reference.local_number(), None));
    };
    let identities = DraftIdentityStore::discover(reference.repository())?;
    let transaction = identities.begin_transaction(reference.repository())?;
    if let Some(identity) = transaction.resolve(temporary_id) {
        require_issue(replica, identity.issue_number, &reference.stable_key())?;
        return Ok((identity.issue_number, None));
    }
    let create = draft_create_operation(outbox, temporary_id)
        .ok_or_else(|| PendingCommentCreateError::MissingIssue(reference.stable_key()))?;
    Ok((temporary_id.synthetic_number(), Some(create.to_owned())))
}

fn draft_create_operation(
    outbox: &PendingMutationOutbox,
    temporary_id: TemporaryIssueId,
) -> Option<&str> {
    outbox.operations().iter().find_map(|operation| {
        operation
            .issue_create_view()
            .filter(|create| create.temporary_id == temporary_id)
            .map(|_| operation.id())
    })
}

fn require_issue(
    replica: &LocalReplica,
    issue_number: u64,
    key: &str,
) -> Result<(), PendingCommentCreateError> {
    if replica
        .issues
        .iter()
        .any(|issue| issue.number == issue_number)
    {
        Ok(())
    } else {
        Err(PendingCommentCreateError::MissingIssue(key.to_owned()))
    }
}

#[derive(Debug, Error)]
pub(crate) enum PendingCommentCreateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    #[error(transparent)]
    WorkingGraph(#[from] WorkingGraphError),
    #[error(transparent)]
    DraftIdentity(#[from] DraftIdentityError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error("comment body cannot be empty")]
    EmptyBody,
    #[error("cannot comment because {0} is absent from the Local replica and Draft outbox")]
    MissingIssue(String),
}
