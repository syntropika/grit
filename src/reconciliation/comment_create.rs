use super::*;
use crate::operation_marker;

impl ReconciliationPass<'_, '_, '_, '_> {
    pub(super) fn reconcile_comment_create_operation(
        &mut self,
        operation: &PendingMutation,
        blocked_by: Vec<String>,
    ) -> Result<(), ReconciliationError> {
        if !blocked_by.is_empty() {
            self.record_comment_create(
                operation,
                Classification::TransitivelyBlocked,
                Outcome::TransitivelyBlocked,
                None,
                blocked_by,
                None,
            );
            return Ok(());
        }
        let comment = operation
            .comment_create_view()
            .expect("comment-create reconciler receives a comment-create mutation");
        match comment.state {
            CommentCreateState::Created { remote } => {
                self.record_comment_create(
                    operation,
                    Classification::Applicable,
                    Outcome::Checkpointed,
                    Some(remote),
                    Vec::new(),
                    None,
                );
                Ok(())
            }
            CommentCreateState::AwaitingMarker { .. } => {
                self.recover_comment_marker(operation, comment.marker, comment.issue.number())
            }
            CommentCreateState::Pending => {
                self.transaction.checkpoint_comment_create_state(
                    self.repository,
                    operation.id(),
                    CommentCreateState::AwaitingMarker {
                        error: "comment request has not yet produced a verified identity"
                            .to_owned(),
                    },
                )?;
                self.requires_final_refresh = true;
                match self.client.create_comment(
                    self.repository,
                    comment.issue.number(),
                    comment.body,
                    comment.marker,
                ) {
                    Ok(remote) => self.accept_remote_comment(operation, &remote, Outcome::Applied),
                    Err(error) => {
                        let message = error.to_string();
                        self.transaction.checkpoint_comment_create_state(
                            self.repository,
                            operation.id(),
                            CommentCreateState::AwaitingMarker {
                                error: message.clone(),
                            },
                        )?;
                        self.record_comment_create(
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

    fn recover_comment_marker(
        &mut self,
        operation: &PendingMutation,
        marker: &str,
        issue_number: u64,
    ) -> Result<(), ReconciliationError> {
        self.requires_final_refresh = true;
        match operation_marker::classify(self.comment_marker_matches.get(marker).map(Vec::as_slice))
        {
            operation_marker::Recovery::Unique(remote) if remote.issue_number == issue_number => {
                self.accept_remote_comment(operation, &remote, Outcome::AlreadySatisfied)
            }
            operation_marker::Recovery::Missing => {
                let message =
                    "no GitHub comment contains the persisted Operation marker; create remains unresolved"
                        .to_owned();
                self.record_comment_create(
                    operation,
                    Classification::Applicable,
                    Outcome::Failed,
                    None,
                    Vec::new(),
                    Some(message),
                );
                Ok(())
            }
            operation_marker::Recovery::Unique(remote) => {
                let message = format!(
                    "the GitHub comment containing the persisted Operation marker belongs to Issue #{} instead of Issue #{issue_number}; create remains unresolved",
                    remote.issue_number
                );
                self.record_comment_create(
                    operation,
                    Classification::Conflicting,
                    Outcome::Failed,
                    None,
                    Vec::new(),
                    Some(message),
                );
                Ok(())
            }
            operation_marker::Recovery::Ambiguous(count) => {
                let message = format!(
                    "{} GitHub comments contain the persisted Operation marker; create remains unresolved",
                    count
                );
                self.record_comment_create(
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

    fn accept_remote_comment(
        &mut self,
        operation: &PendingMutation,
        remote: &CommentIdentity,
        outcome: Outcome,
    ) -> Result<(), ReconciliationError> {
        let comment = operation
            .comment_create_view()
            .expect("remote comment belongs to a comment-create mutation");
        if remote.issue_number != comment.issue.number() {
            self.record_comment_create(
                operation,
                Classification::Conflicting,
                Outcome::Failed,
                None,
                Vec::new(),
                Some("GitHub returned a comment for a different Issue".to_owned()),
            );
            return Ok(());
        }
        self.transaction.checkpoint_comment_create_state(
            self.repository,
            operation.id(),
            CommentCreateState::Created {
                remote: remote.clone(),
            },
        )?;
        self.record_comment_create(
            operation,
            if outcome == Outcome::AlreadySatisfied {
                Classification::AlreadySatisfied
            } else {
                Classification::Applicable
            },
            outcome,
            Some(remote),
            Vec::new(),
            None,
        );
        Ok(())
    }

    fn record_comment_create(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        outcome: Outcome,
        remote: Option<&CommentIdentity>,
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
    let Some(CommentCreateState::Created { remote }) =
        operation.comment_create_view().map(|comment| comment.state)
    else {
        return false;
    };
    let verified = final_replica.issues.iter().any(|issue| {
        issue.number == remote.issue_number
            && issue
                .comments
                .iter()
                .any(|comment| comment.id == remote.id && comment.node_id == remote.node_id)
    });
    if verified {
        return true;
    }
    let error = "final GitHub synchronization did not verify the created comment";
    state_updates.insert(
        operation.id().to_owned(),
        MutationStateUpdate::CommentCreate(CommentCreateState::AwaitingMarker {
            error: error.to_owned(),
        }),
    );
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
    remote: Option<&CommentIdentity>,
    blocked_by: Vec<String>,
    error: Option<String>,
) -> OperationResult {
    let comment = operation
        .comment_create_view()
        .expect("comment-create result is built from a comment-create mutation");
    OperationResult {
        id: operation.id().to_owned(),
        issue_number: Some(comment.issue.number()),
        temporary_id: comment.issue.temporary_id(),
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        details: OperationDetails::CommentCreate {
            remote: remote.cloned(),
        },
        blocked_by,
        error,
    }
}
