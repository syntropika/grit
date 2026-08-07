use super::*;

impl ReconciliationPass<'_, '_, '_, '_> {
    pub(super) fn reconcile_issue_create_operation(
        &mut self,
        operation: &PendingMutation,
        blocked_by: Vec<String>,
    ) -> Result<(), ReconciliationError> {
        debug_assert!(blocked_by.is_empty(), "Issue creates have no prerequisites");

        let create = operation
            .issue_create_view()
            .expect("Issue-create reconciler receives an Issue-create mutation");
        if let Some(identity) = self.identity_transaction.resolve(create.temporary_id) {
            return self.accept_mapping(operation, &identity, Outcome::Checkpointed);
        }

        match create.state {
            IssueCreateState::Mapped {
                issue_id,
                issue_node_id,
                issue_number,
                issue_url,
            } => {
                let identity = DraftIdentity {
                    temporary_id: create.temporary_id,
                    synthetic_number: create.synthetic_number,
                    issue_id: *issue_id,
                    issue_node_id: issue_node_id.clone(),
                    issue_number: *issue_number,
                    issue_url: issue_url.clone(),
                };
                self.record_issue_create(
                    operation,
                    Classification::Applicable,
                    Outcome::Checkpointed,
                    Some(&identity),
                    Vec::new(),
                    None,
                );
                Ok(())
            }
            IssueCreateState::AwaitingMarker { .. } => {
                self.recover_marker(operation, create.marker)
            }
            IssueCreateState::Pending => {
                self.transaction.checkpoint_issue_create_state(
                    self.repository,
                    operation.id(),
                    IssueCreateState::AwaitingMarker {
                        error: "create request has not yet produced a verified identity".to_owned(),
                    },
                )?;
                self.requires_final_refresh = true;
                match self.client.create_issue(
                    self.repository,
                    create.title,
                    create.body,
                    create.marker,
                ) {
                    Ok(remote) => self.accept_remote_mapping(operation, &remote, Outcome::Applied),
                    Err(error) => {
                        let message = error.to_string();
                        self.transaction.checkpoint_issue_create_state(
                            self.repository,
                            operation.id(),
                            IssueCreateState::AwaitingMarker {
                                error: message.clone(),
                            },
                        )?;
                        self.record_issue_create(
                            operation,
                            Classification::Applicable,
                            Outcome::Failed,
                            None,
                            Vec::new(),
                            Some(message),
                        );
                        Ok(())
                    }
                }
            }
        }
    }

    fn recover_marker(
        &mut self,
        operation: &PendingMutation,
        marker: &str,
    ) -> Result<(), ReconciliationError> {
        self.requires_final_refresh = true;
        let matches = self.marker_matches.get(marker).cloned().unwrap_or_default();
        match matches.as_slice() {
            [remote] => self.accept_remote_mapping(operation, remote, Outcome::AlreadySatisfied),
            [] => {
                let message =
                    "no GitHub Issue contains the persisted Operation marker; create remains unresolved"
                        .to_owned();
                self.record_issue_create(
                    operation,
                    Classification::Applicable,
                    Outcome::Failed,
                    None,
                    Vec::new(),
                    Some(message),
                );
                Ok(())
            }
            _ => {
                let message = format!(
                    "{} GitHub Issues contain the persisted Operation marker; create remains unresolved",
                    matches.len()
                );
                self.record_issue_create(
                    operation,
                    Classification::Conflicting,
                    Outcome::Failed,
                    None,
                    Vec::new(),
                    Some(message),
                );
                Ok(())
            }
        }
    }

    fn accept_remote_mapping(
        &mut self,
        operation: &PendingMutation,
        remote: &crate::github::CreatedIssueIdentity,
        outcome: Outcome,
    ) -> Result<(), ReconciliationError> {
        let create = operation
            .issue_create_view()
            .expect("remote mapping belongs to an Issue-create mutation");
        match self
            .identity_transaction
            .record(create.temporary_id, create.synthetic_number, remote)
        {
            Ok(identity) => self.accept_mapping(operation, &identity, outcome),
            Err(error) if error.is_mapping_conflict() => {
                self.record_issue_create(
                    operation,
                    Classification::Conflicting,
                    Outcome::Failed,
                    None,
                    Vec::new(),
                    Some(error.to_string()),
                );
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn accept_mapping(
        &mut self,
        operation: &PendingMutation,
        identity: &DraftIdentity,
        outcome: Outcome,
    ) -> Result<(), ReconciliationError> {
        self.remote
            .entry(identity.issue_number)
            .or_insert_with(|| RemotePriority {
                logical: LogicalPriority::Unspecified,
                canonical_labels: Vec::new(),
            });
        let create = operation
            .issue_create_view()
            .expect("Draft mapping belongs to an Issue-create mutation");
        self.remote_issues
            .entry(identity.issue_number)
            .or_insert_with(|| Issue {
                id: identity.issue_id,
                node_id: identity.issue_node_id.clone(),
                number: identity.issue_number,
                url: identity.issue_url.clone(),
                title: create.title.to_owned(),
                body: create.body.to_owned(),
                state: "open".to_owned(),
                state_reason: None,
                author: None,
                assignees: Vec::new(),
                labels: Vec::new(),
                comments: Vec::new(),
                created_at: create.created_at.to_owned(),
                updated_at: create.created_at.to_owned(),
                closed_at: None,
                identity: crate::model::IssueIdentityState::MappedDraft(create.temporary_id),
            });
        self.transaction
            .checkpoint_draft_mapping(self.repository, operation.id(), identity)?;
        self.record_issue_create(
            operation,
            if outcome == Outcome::AlreadySatisfied {
                Classification::AlreadySatisfied
            } else {
                Classification::Applicable
            },
            outcome,
            Some(identity),
            Vec::new(),
            None,
        );
        Ok(())
    }

    fn record_issue_create(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        outcome: Outcome,
        identity: Option<&DraftIdentity>,
        blocked_by: Vec<String>,
        error: Option<String>,
    ) {
        self.results.push(result_for(
            operation,
            classification,
            outcome,
            identity,
            blocked_by,
            error,
        ));
    }
}

pub(super) fn verify_terminal(
    operation: &PendingMutation,
    final_replica: &LocalReplica,
    results: &mut [OperationResult],
    _state_updates: &mut BTreeMap<String, MutationStateUpdate>,
) -> bool {
    let Some(IssueCreateState::Mapped {
        issue_id,
        issue_node_id,
        issue_number,
        issue_url: _,
    }) = operation.issue_create_state()
    else {
        return false;
    };
    let verified = final_replica.issues.iter().any(|issue| {
        issue.id == *issue_id && issue.node_id == *issue_node_id && issue.number == *issue_number
    });
    if verified {
        return true;
    }
    let error = "final GitHub synchronization did not verify the mapped Draft Issue";
    if let Some(result) = results
        .iter_mut()
        .find(|result| result.id == operation.id())
    {
        result.classification = Classification::Applicable;
        result.outcome = Outcome::Failed;
        result.error = Some(error.to_owned());
    }
    false
}

fn result_for(
    operation: &PendingMutation,
    classification: Classification,
    outcome: Outcome,
    identity: Option<&DraftIdentity>,
    blocked_by: Vec<String>,
    error: Option<String>,
) -> OperationResult {
    let create = operation
        .issue_create_view()
        .expect("Issue-create result is built from an Issue-create mutation");
    OperationResult {
        id: operation.id().to_owned(),
        issue_number: identity.map(|identity| identity.issue_number),
        temporary_id: Some(create.temporary_id),
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        details: OperationDetails::IssueCreate {
            remote: identity.map(|identity| DraftIssueResult {
                issue_id: identity.issue_id,
                issue_node_id: identity.issue_node_id.clone(),
                issue_number: identity.issue_number,
                issue_url: identity.issue_url.clone(),
            }),
        },
        blocked_by,
        error,
    }
}
