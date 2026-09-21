use super::*;

impl ReconciliationPass<'_, '_, '_, '_> {
    pub(super) fn reconcile_issue_field_operation(
        &mut self,
        operation: &PendingMutation,
        blocked_by: Vec<String>,
    ) -> Result<(), ReconciliationError> {
        if !blocked_by.is_empty() {
            self.stage_issue_field_state(
                operation.id(),
                IssueFieldMutationState::TransitivelyBlocked {
                    blocked_by: blocked_by.clone(),
                },
            )?;
            self.record_issue_field(
                operation,
                Classification::TransitivelyBlocked,
                Outcome::TransitivelyBlocked,
                self.observed_value(operation),
                blocked_by,
                None,
            );
            return Ok(());
        }

        let update = operation
            .issue_field_update_view()
            .expect("Issue-field reconciler receives an Issue-field mutation");
        let Some(remote_issue) = self.remote_issues.get(&update.issue_number) else {
            let message = format!(
                "GitHub Issue #{} is missing during Issue-field reconciliation",
                update.issue_number
            );
            self.stage_issue_field_state(
                operation.id(),
                IssueFieldMutationState::Failed {
                    error: message.clone(),
                },
            )?;
            self.record_issue_field(
                operation,
                Classification::Applicable,
                Outcome::Failed,
                None,
                Vec::new(),
                Some(message),
            );
            return Ok(());
        };
        let remote = IssueFieldValue::from_issue(update.field, remote_issue)?;
        match update.state {
            IssueFieldMutationState::Applied => self.record_issue_field(
                operation,
                Classification::Applicable,
                Outcome::Checkpointed,
                Some(remote),
                Vec::new(),
                None,
            ),
            IssueFieldMutationState::AlreadySatisfied => self.record_issue_field(
                operation,
                Classification::AlreadySatisfied,
                Outcome::AlreadySatisfied,
                Some(remote),
                Vec::new(),
                None,
            ),
            IssueFieldMutationState::ResolvedRemote { remote: accepted } => {
                if *accepted != remote {
                    self.checkpoint_issue_field_state(
                        operation.id(),
                        IssueFieldMutationState::ResolvedRemote {
                            remote: remote.clone(),
                        },
                    )?;
                }
                self.record_issue_field(
                    operation,
                    Classification::AlreadySatisfied,
                    Outcome::ResolvedRemote,
                    Some(remote),
                    Vec::new(),
                    None,
                )
            }
            IssueFieldMutationState::Pending
            | IssueFieldMutationState::Conflicting { .. }
            | IssueFieldMutationState::Failed { .. }
            | IssueFieldMutationState::TransitivelyBlocked { .. } => {
                self.classify_issue_field(operation, remote)?;
            }
        }
        Ok(())
    }

    fn classify_issue_field(
        &mut self,
        operation: &PendingMutation,
        remote: IssueFieldValue,
    ) -> Result<(), ReconciliationError> {
        let update = operation
            .issue_field_update_view()
            .expect("Issue-field classification receives an Issue-field mutation");
        match classify_scalar(update.base, update.desired, &remote) {
            ScalarClassification::AlreadySatisfied => {
                self.stage_issue_field_state(
                    operation.id(),
                    IssueFieldMutationState::AlreadySatisfied,
                )?;
                self.record_issue_field(
                    operation,
                    Classification::AlreadySatisfied,
                    Outcome::AlreadySatisfied,
                    Some(remote),
                    Vec::new(),
                    None,
                );
                return Ok(());
            }
            ScalarClassification::Conflicting => {
                self.stage_issue_field_state(
                    operation.id(),
                    IssueFieldMutationState::Conflicting {
                        remote: remote.clone(),
                    },
                )?;
                self.record_issue_field(
                    operation,
                    Classification::Conflicting,
                    Outcome::Conflicting,
                    Some(remote),
                    Vec::new(),
                    None,
                );
                return Ok(());
            }
            ScalarClassification::Applicable => {}
        }

        self.requires_final_refresh = true;
        match self.client.patch_issue_field(
            self.repository,
            update.issue_number,
            update.field,
            update.desired,
        ) {
            Ok(()) => {
                self.checkpoint_issue_field_state(
                    operation.id(),
                    IssueFieldMutationState::Applied,
                )?;
                let issue = self
                    .remote_issues
                    .get_mut(&update.issue_number)
                    .expect("remote Issue was observed before its field write");
                update.desired.apply_to(issue);
                self.record_issue_field(
                    operation,
                    Classification::Applicable,
                    Outcome::Applied,
                    Some(update.desired.clone()),
                    Vec::new(),
                    None,
                );
            }
            Err(error) => {
                let message = error.to_string();
                self.checkpoint_issue_field_state(
                    operation.id(),
                    IssueFieldMutationState::Failed {
                        error: message.clone(),
                    },
                )?;
                self.record_issue_field(
                    operation,
                    Classification::Applicable,
                    Outcome::Failed,
                    Some(remote),
                    Vec::new(),
                    Some(message),
                );
            }
        }
        Ok(())
    }

    fn observed_value(&self, operation: &PendingMutation) -> Option<IssueFieldValue> {
        let update = operation.issue_field_update_view()?;
        self.remote_issues
            .get(&update.issue_number)
            .and_then(|issue| IssueFieldValue::from_issue(update.field, issue).ok())
    }

    fn stage_issue_field_state(
        &mut self,
        operation_id: &str,
        state: IssueFieldMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .stage_issue_field_state(operation_id, state)?;
        Ok(())
    }

    fn checkpoint_issue_field_state(
        &mut self,
        operation_id: &str,
        state: IssueFieldMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .checkpoint_issue_field_state(self.repository, operation_id, state)?;
        Ok(())
    }

    fn record_issue_field(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        outcome: Outcome,
        remote: Option<IssueFieldValue>,
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

pub(super) fn verify_terminal(
    operation: &PendingMutation,
    final_replica: &LocalReplica,
    results: &mut [OperationResult],
    state_updates: &mut BTreeMap<String, MutationStateUpdate>,
) -> bool {
    let update = operation
        .issue_field_update_view()
        .expect("Issue-field verification receives an Issue-field mutation");
    let expected = match update.state {
        IssueFieldMutationState::Applied | IssueFieldMutationState::AlreadySatisfied => {
            update.desired
        }
        IssueFieldMutationState::ResolvedRemote { remote } => remote,
        _ => return false,
    };
    let actual = final_replica
        .issues
        .iter()
        .find(|issue| issue.number == update.issue_number)
        .and_then(|issue| IssueFieldValue::from_issue(update.field, issue).ok());
    if actual.as_ref() == Some(expected) {
        if let Some(result) = results
            .iter_mut()
            .find(|result| result.id == operation.id())
            && let OperationDetails::IssueFieldUpdate { remote, .. } = &mut result.details
        {
            *remote = actual;
        }
        return true;
    }
    let message = format!(
        "final GitHub synchronization did not verify the {:?} field",
        update.field
    );
    state_updates.insert(
        operation.id().to_owned(),
        MutationStateUpdate::IssueField(IssueFieldMutationState::Failed {
            error: message.clone(),
        }),
    );
    if let Some(result) = results
        .iter_mut()
        .find(|result| result.id == operation.id())
    {
        result.classification = Classification::Applicable;
        result.outcome = Outcome::Failed;
        result.error = Some(message);
        if let OperationDetails::IssueFieldUpdate { remote, .. } = &mut result.details {
            *remote = actual;
        }
    }
    false
}

fn result_for(
    operation: &PendingMutation,
    classification: Classification,
    outcome: Outcome,
    remote: Option<IssueFieldValue>,
    blocked_by: Vec<String>,
    error: Option<String>,
) -> OperationResult {
    let update = operation
        .issue_field_update_view()
        .expect("Issue-field result is built from an Issue-field mutation");
    OperationResult {
        id: operation.id().to_owned(),
        issue_number: Some(update.issue_number),
        temporary_id: update.temporary_id,
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        details: OperationDetails::IssueFieldUpdate {
            field: update.field,
            base: update.base.clone(),
            local: update.desired.clone(),
            remote,
        },
        blocked_by,
        error,
    }
}
