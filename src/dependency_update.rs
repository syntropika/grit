use thiserror::Error;

use crate::{
    model::{DependencyEdgeKey, DependencyPresence, LocalReplica},
    outbox::{OutboxError, OutboxStore, PendingMutation},
    repository::IssueReference,
    store::{ReplicaStore, StoreError},
    working_graph::{WorkingGraph, WorkingGraphError},
};

pub(crate) struct PendingDependencyUpdateResult {
    pub(crate) operation: PendingMutation,
    pub(crate) working_input_hash: String,
    pub(crate) replica: LocalReplica,
}

pub(crate) fn queue(
    blocked: &IssueReference,
    blocker: &IssueReference,
    desired: DependencyPresence,
) -> Result<PendingDependencyUpdateResult, PendingDependencyUpdateError> {
    if blocked
        .repository()
        .full_name()
        .eq_ignore_ascii_case(blocker.repository().full_name())
        && blocked.number() == blocker.number()
    {
        return Err(PendingDependencyUpdateError::SelfDependency(
            blocked.stable_key(),
        ));
    }

    let outbox_store = OutboxStore::discover(blocked.repository())?;
    let transaction = outbox_store.begin_transaction(blocked.repository())?;
    let replica = ReplicaStore::discover(blocked.repository())?.load(blocked.repository())?;
    require_issue(&replica, blocked)?;
    if blocked
        .repository()
        .full_name()
        .eq_ignore_ascii_case(blocker.repository().full_name())
    {
        require_issue(&replica, blocker)?;
    }

    let edge = DependencyEdgeKey::new(
        blocked.repository().full_name(),
        blocked.number(),
        blocker.repository().full_name(),
        blocker.number(),
    );
    WorkingGraph::validate_references(&replica, transaction.outbox())?;

    let depends_on = transaction
        .outbox()
        .latest_operation_for_dependency(&edge)
        .map(|operation| vec![operation.to_owned()])
        .unwrap_or_default();
    let operation =
        PendingMutation::dependency_update(blocked.repository(), edge, desired, depends_on);
    let next_outbox = transaction.append(blocked.repository(), operation.clone())?;
    let next_working = WorkingGraph::project(&replica, &next_outbox)?;
    debug_assert_eq!(
        next_working.replica().has_dependency(blocked, blocker),
        desired.is_present()
    );

    Ok(PendingDependencyUpdateResult {
        operation,
        working_input_hash: next_working.input_hash().to_owned(),
        replica,
    })
}

fn require_issue(
    replica: &LocalReplica,
    reference: &IssueReference,
) -> Result<(), PendingDependencyUpdateError> {
    if replica
        .issues
        .iter()
        .any(|issue| issue.number == reference.number())
    {
        Ok(())
    } else {
        Err(PendingDependencyUpdateError::MissingIssue(
            reference.stable_key(),
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
    #[error("cannot queue a native Dependency because {0} is absent from the Local replica")]
    MissingIssue(String),
    #[error("cannot make {0} depend on itself")]
    SelfDependency(String),
}
