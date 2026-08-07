use super::*;
use crate::{github::MetadataChange, metadata::MetadataSetTarget, outbox::SetMutationState};

impl ReconciliationPass<'_, '_, '_, '_> {
    pub(super) fn reconcile_metadata_set_operation(
        &mut self,
        operation: &PendingMutation,
        blocked_by: Vec<String>,
    ) -> Result<(), ReconciliationError> {
        if !blocked_by.is_empty() {
            self.stage_metadata_state(
                operation.id(),
                SetMutationState::TransitivelyBlocked {
                    blocked_by: blocked_by.clone(),
                },
            )?;
            self.record_metadata(
                operation,
                Classification::TransitivelyBlocked,
                Outcome::TransitivelyBlocked,
                None,
                blocked_by,
                None,
            );
            return Ok(());
        }
        let (target, desired, state) = {
            let update = operation
                .metadata_set_update_view()
                .expect("metadata reconciler receives a metadata mutation");
            (update.target.clone(), update.desired, update.state.clone())
        };
        let observed = observe(self, &target)?;
        match state {
            SetMutationState::Applied => self.record_metadata(
                operation,
                Classification::Applicable,
                Outcome::Checkpointed,
                Some(observed),
                Vec::new(),
                None,
            ),
            SetMutationState::AlreadySatisfied => self.record_metadata(
                operation,
                Classification::AlreadySatisfied,
                Outcome::AlreadySatisfied,
                Some(observed),
                Vec::new(),
                None,
            ),
            SetMutationState::Pending
            | SetMutationState::Applying { .. }
            | SetMutationState::Failed { .. }
            | SetMutationState::TransitivelyBlocked { .. } => {
                if observed == desired.is_present() {
                    self.stage_metadata_state(operation.id(), SetMutationState::AlreadySatisfied)?;
                    self.record_metadata(
                        operation,
                        Classification::AlreadySatisfied,
                        Outcome::AlreadySatisfied,
                        Some(observed),
                        Vec::new(),
                        None,
                    );
                } else {
                    self.apply_metadata(operation, &target, desired, observed)?;
                }
            }
        }
        Ok(())
    }

    fn apply_metadata(
        &mut self,
        operation: &PendingMutation,
        target: &MetadataSetTarget,
        desired: SetPresence,
        observed: bool,
    ) -> Result<(), ReconciliationError> {
        self.checkpoint_metadata_state(
            operation.id(),
            SetMutationState::Applying { last_error: None },
        )?;
        self.requires_final_refresh = true;
        let result = match target {
            MetadataSetTarget::GenericLabel { issue, label } => self.client.mutate_generic_label(
                &issue_reference(self.repository, issue.number()),
                label,
                desired,
            ),
            MetadataSetTarget::ParentRelationship { parent, child } => {
                let child_id = self
                    .remote_issues
                    .get(&child.number())
                    .ok_or(ReconciliationError::MissingMetadataIssue(child.number()))?
                    .id;
                self.client.set_parent_relationship(
                    &issue_reference(self.repository, parent.number()),
                    child_id,
                    desired,
                )
            }
        };
        match result {
            Ok(change) => {
                remember_observation(self, target, desired.is_present());
                self.checkpoint_metadata_state(operation.id(), SetMutationState::Applied)?;
                self.record_metadata(
                    operation,
                    Classification::Applicable,
                    outcome(change),
                    Some(desired.is_present()),
                    Vec::new(),
                    None,
                );
            }
            Err(error) => {
                let message = error.to_string();
                self.checkpoint_metadata_state(
                    operation.id(),
                    SetMutationState::Applying {
                        last_error: Some(message.clone()),
                    },
                )?;
                self.record_metadata(
                    operation,
                    Classification::Applicable,
                    Outcome::Failed,
                    Some(observed),
                    Vec::new(),
                    Some(message),
                );
            }
        }
        Ok(())
    }

    fn checkpoint_metadata_state(
        &mut self,
        operation_id: &str,
        state: SetMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .checkpoint_metadata_state(self.repository, operation_id, state)?;
        Ok(())
    }

    fn stage_metadata_state(
        &mut self,
        operation_id: &str,
        state: SetMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction.stage_metadata_state(operation_id, state)?;
        Ok(())
    }

    fn record_metadata(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        outcome: Outcome,
        remote_present: Option<bool>,
        blocked_by: Vec<String>,
        error: Option<String>,
    ) {
        self.results.push(result_for(
            operation,
            classification,
            outcome,
            remote_present,
            blocked_by,
            error,
        ));
    }
}

fn observe(
    pass: &mut ReconciliationPass<'_, '_, '_, '_>,
    target: &MetadataSetTarget,
) -> Result<bool, ReconciliationError> {
    if let Some(present) = pass.metadata_presence.get(target) {
        return Ok(*present);
    }
    let present = match target {
        MetadataSetTarget::GenericLabel { issue, label } => {
            let remote = pass
                .remote_issues
                .get(&issue.number())
                .ok_or(ReconciliationError::MissingMetadataIssue(issue.number()))?;
            remote
                .labels
                .iter()
                .any(|candidate| label.matches(&candidate.name))
        }
        MetadataSetTarget::ParentRelationship { parent, child } => {
            let child_id = pass
                .remote_issues
                .get(&child.number())
                .ok_or(ReconciliationError::MissingMetadataIssue(child.number()))?
                .id;
            if !pass.remote_issues.contains_key(&parent.number()) {
                return Err(ReconciliationError::MissingMetadataIssue(parent.number()));
            }
            match pass.remote_sub_issues.entry(parent.number()) {
                std::collections::btree_map::Entry::Occupied(entry) => {
                    entry.get().contains(&child_id)
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let children = pass
                        .client
                        .sub_issue_ids(&issue_reference(pass.repository, parent.number()))?;
                    entry.insert(children).contains(&child_id)
                }
            }
        }
    };
    pass.metadata_presence.insert(target.clone(), present);
    Ok(present)
}

fn remember_observation(
    pass: &mut ReconciliationPass<'_, '_, '_, '_>,
    target: &MetadataSetTarget,
    present: bool,
) {
    pass.metadata_presence.insert(target.clone(), present);
    if let MetadataSetTarget::ParentRelationship { parent, child } = target
        && let Some(child_id) = pass
            .remote_issues
            .get(&child.number())
            .map(|issue| issue.id)
        && let Some(children) = pass.remote_sub_issues.get_mut(&parent.number())
    {
        if present {
            children.insert(child_id);
        } else {
            children.remove(&child_id);
        }
    }
}

pub(super) fn verify_terminal(
    client: &GitHubClient,
    repository: &Repository,
    operation: &PendingMutation,
    final_replica: &LocalReplica,
    results: &mut [OperationResult],
    state_updates: &mut BTreeMap<String, MutationStateUpdate>,
    sub_issues: &mut BTreeMap<u64, BTreeSet<u64>>,
) -> Result<bool, ReconciliationError> {
    let update = operation
        .metadata_set_update_view()
        .expect("metadata verifier receives a metadata mutation");
    let observed = match update.target {
        MetadataSetTarget::GenericLabel { issue, label } => final_replica
            .issues
            .iter()
            .find(|candidate| candidate.number == issue.number())
            .is_some_and(|issue| {
                issue
                    .labels
                    .iter()
                    .any(|candidate| label.matches(&candidate.name))
            }),
        MetadataSetTarget::ParentRelationship { parent, child } => {
            let child_id = final_replica
                .issues
                .iter()
                .find(|issue| issue.number == child.number())
                .ok_or(ReconciliationError::MissingMetadataIssue(child.number()))?
                .id;
            match sub_issues.entry(parent.number()) {
                std::collections::btree_map::Entry::Occupied(entry) => {
                    entry.get().contains(&child_id)
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let children =
                        client.sub_issue_ids(&issue_reference(repository, parent.number()))?;
                    entry.insert(children).contains(&child_id)
                }
            }
        }
    };
    if observed == update.desired.is_present() {
        return Ok(true);
    }
    let error = "final GitHub synchronization did not verify the intended metadata set state";
    state_updates.insert(
        operation.id().to_owned(),
        MutationStateUpdate::MetadataSet(SetMutationState::Applying {
            last_error: Some(error.to_owned()),
        }),
    );
    if let Some(result) = results
        .iter_mut()
        .find(|result| result.id == operation.id())
    {
        result.classification = Classification::Applicable;
        result.outcome = Outcome::Failed;
        result.error = Some(error.to_owned());
        if let OperationDetails::MetadataSetUpdate { remote_present, .. } = &mut result.details {
            *remote_present = Some(observed);
        }
    }
    Ok(false)
}

fn outcome(change: MetadataChange) -> Outcome {
    match change {
        MetadataChange::Added | MetadataChange::Removed => Outcome::Applied,
        MetadataChange::AlreadyPresent | MetadataChange::AlreadyAbsent => Outcome::AlreadySatisfied,
    }
}

fn issue_reference(repository: &Repository, number: u64) -> IssueReference {
    IssueReference::parse(&format!("{}#{number}", repository.full_name()))
        .expect("validated Repository and Issue number form a reference")
}

fn result_for(
    operation: &PendingMutation,
    classification: Classification,
    outcome: Outcome,
    remote_present: Option<bool>,
    blocked_by: Vec<String>,
    error: Option<String>,
) -> OperationResult {
    let update = operation
        .metadata_set_update_view()
        .expect("metadata result receives a metadata mutation");
    OperationResult {
        id: operation.id().to_owned(),
        issue_number: Some(update.target.primary_number()),
        temporary_id: None,
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        details: OperationDetails::MetadataSetUpdate {
            target: update.target.clone(),
            desired_present: update.desired.is_present(),
            remote_present,
        },
        blocked_by,
        error,
    }
}
