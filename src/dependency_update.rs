use thiserror::Error;

use crate::{
    draft_identity::{DraftIdentityError, DraftIdentityStore},
    model::{DependencyEdgeKey, DependencyPresence, LocalReplica, TemporaryIssueId},
    outbox::{OutboxError, OutboxStore, PendingMutation, PendingMutationOutbox},
    repository::PendingIssueReference,
    store::{ReplicaStore, StoreError},
    working_graph::{WorkingGraph, WorkingGraphError},
};

pub(crate) struct PendingDependencyUpdateResult {
    pub(crate) operation: PendingMutation,
    pub(crate) working_input_hash: String,
    pub(crate) replica: LocalReplica,
}

pub(crate) fn queue(
    blocked: &PendingIssueReference,
    blocker: &PendingIssueReference,
    desired: DependencyPresence,
) -> Result<PendingDependencyUpdateResult, PendingDependencyUpdateError> {
    if !blocked
        .repository()
        .full_name()
        .eq_ignore_ascii_case(blocker.repository().full_name())
        && (blocked.temporary_id().is_some() || blocker.temporary_id().is_some())
    {
        return Err(PendingDependencyUpdateError::CrossRepositoryDraft);
    }

    let outbox_store = OutboxStore::discover(blocked.repository())?;
    let transaction = outbox_store.begin_transaction(blocked.repository())?;
    let replica = ReplicaStore::discover(blocked.repository())?.load(blocked.repository())?;
    let identity_store = DraftIdentityStore::discover(blocked.repository())?;
    let identity_transaction = identity_store.begin_transaction(blocked.repository())?;
    let (blocked_number, blocked_create) = resolve_reference(
        &identity_transaction,
        &replica,
        transaction.outbox(),
        blocked,
        true,
    )?;
    let internal = blocked
        .repository()
        .full_name()
        .eq_ignore_ascii_case(blocker.repository().full_name());
    let (blocker_number, blocker_create) = resolve_reference(
        &identity_transaction,
        &replica,
        transaction.outbox(),
        blocker,
        internal,
    )?;

    if blocked
        .repository()
        .full_name()
        .eq_ignore_ascii_case(blocker.repository().full_name())
        && blocked_number == blocker_number
    {
        return Err(PendingDependencyUpdateError::SelfDependency(
            blocked.stable_key(),
        ));
    }

    let edge = DependencyEdgeKey::with_temporary_aliases(
        blocked.repository().full_name(),
        blocked_number,
        blocked.temporary_id(),
        blocker.repository().full_name(),
        blocker_number,
        blocker.temporary_id(),
    );
    WorkingGraph::validate_references(&replica, transaction.outbox())?;

    let mut depends_on = Vec::new();
    depends_on.extend(blocked_create);
    depends_on.extend(blocker_create);
    if let Some(operation) = transaction.outbox().latest_operation_for_dependency(&edge) {
        depends_on.push(operation.to_owned());
    }
    depends_on.sort_by_key(|operation_id| {
        transaction
            .outbox()
            .operations()
            .iter()
            .position(|operation| operation.id() == operation_id)
            .unwrap_or(usize::MAX)
    });
    depends_on.dedup();

    let operation =
        PendingMutation::dependency_update(blocked.repository(), edge, desired, depends_on);
    let next_outbox = transaction.append(blocked.repository(), operation.clone())?;
    let next_working = WorkingGraph::project(&replica, &next_outbox)?;

    Ok(PendingDependencyUpdateResult {
        operation,
        working_input_hash: next_working.input_hash().to_owned(),
        replica,
    })
}

fn resolve_reference(
    identity_transaction: &crate::draft_identity::DraftIdentityTransaction<'_>,
    replica: &LocalReplica,
    outbox: &PendingMutationOutbox,
    reference: &PendingIssueReference,
    require_local: bool,
) -> Result<(u64, Option<String>), PendingDependencyUpdateError> {
    let Some(temporary_id) = reference.temporary_id() else {
        if require_local {
            require_issue(replica, reference.local_number(), &reference.stable_key())?;
        }
        return Ok((reference.local_number(), None));
    };
    if let Some(identity) = identity_transaction.resolve(temporary_id) {
        require_issue(replica, identity.issue_number, &reference.stable_key())?;
        return Ok((identity.issue_number, None));
    }
    let create = draft_create_operation(outbox, temporary_id)
        .ok_or_else(|| PendingDependencyUpdateError::MissingIssue(reference.stable_key()))?;
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
    stable_key: &str,
) -> Result<(), PendingDependencyUpdateError> {
    if replica
        .issues
        .iter()
        .any(|issue| issue.number == issue_number)
    {
        Ok(())
    } else {
        Err(PendingDependencyUpdateError::MissingIssue(
            stable_key.to_owned(),
        ))
    }
}

#[derive(Debug, Error)]
pub(crate) enum PendingDependencyUpdateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    #[error(transparent)]
    WorkingGraph(#[from] WorkingGraphError),
    #[error(transparent)]
    DraftIdentity(#[from] DraftIdentityError),
    #[error(
        "cannot queue a native Dependency because {0} is absent from the Local replica and Draft outbox"
    )]
    MissingIssue(String),
    #[error("cannot make {0} depend on itself")]
    SelfDependency(String),
    #[error("Draft Issue Dependencies must stay within one Repository")]
    CrossRepositoryDraft,
}
