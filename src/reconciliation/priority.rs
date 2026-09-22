use super::*;

impl ReconciliationPass<'_, '_, '_, '_> {
    pub(super) fn reconcile_priority_operation(
        &mut self,
        operation: &PendingMutation,
        blocked_by: Vec<String>,
    ) -> Result<(), ReconciliationError> {
        if !blocked_by.is_empty() {
            self.stage_priority_state(
                operation.id(),
                PriorityMutationState::TransitivelyBlocked {
                    blocked_by: blocked_by.clone(),
                },
            )?;
            self.record_priority(
                operation,
                Classification::TransitivelyBlocked,
                Outcome::TransitivelyBlocked,
                self.observed_priority(operation.issue_number()),
                blocked_by,
                None,
            );
            return Ok(());
        }

        let Some(remote) = self.remote.get(&operation.issue_number()).cloned() else {
            return self.record_missing_priority_issue(operation);
        };
        let state = self
            .transaction
            .operation(operation.id())
            .expect("known operation")
            .priority_state()
            .expect("Priority operation has Priority state")
            .clone();
        self.reconcile_state(operation, state, remote)
    }

    fn reconcile_state(
        &mut self,
        operation: &PendingMutation,
        state: PriorityMutationState,
        mut remote: RemotePriority,
    ) -> Result<(), ReconciliationError> {
        let (_, desired) = operation
            .priority_values()
            .expect("Priority reconciliation receives a Priority mutation");
        match state {
            PriorityMutationState::Conflicting { .. } => {
                self.classify_pending(operation, &mut remote)
            }
            PriorityMutationState::Applied => {
                self.record_terminal(operation, Classification::Applicable, remote.logical)
            }
            PriorityMutationState::AlreadySatisfied => {
                self.record_terminal(operation, Classification::AlreadySatisfied, remote.logical)
            }
            PriorityMutationState::ResolvedRemote { .. } => {
                self.record_priority(
                    operation,
                    Classification::AlreadySatisfied,
                    Outcome::ResolvedRemote,
                    Some(remote.logical),
                    Vec::new(),
                    None,
                );
                Ok(())
            }
            PriorityMutationState::Applying {
                expected_labels,
                remaining_writes,
                ..
            } => {
                if remote.canonical_labels != expected_labels {
                    if remote.logical == *desired {
                        self.checkpoint_priority_state(
                            operation.id(),
                            PriorityMutationState::Applied,
                        )?;
                        self.record_priority(
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
            PriorityMutationState::Pending
            | PriorityMutationState::Failed { .. }
            | PriorityMutationState::TransitivelyBlocked { .. } => {
                self.classify_pending(operation, &mut remote)
            }
        }
    }

    fn classify_pending(
        &mut self,
        operation: &PendingMutation,
        remote: &mut RemotePriority,
    ) -> Result<(), ReconciliationError> {
        let (base, desired) = operation
            .priority_values()
            .expect("Priority classification receives a Priority mutation");
        match classify_scalar(base, desired, &remote.logical) {
            ScalarClassification::AlreadySatisfied => {
                self.stage_priority_state(operation.id(), PriorityMutationState::AlreadySatisfied)?;
                self.record_priority(
                    operation,
                    Classification::AlreadySatisfied,
                    Outcome::AlreadySatisfied,
                    Some(remote.logical.clone()),
                    Vec::new(),
                    None,
                );
                Ok(())
            }
            ScalarClassification::Conflicting => {
                self.record_conflict(operation, remote.logical.clone())
            }
            ScalarClassification::Applicable => {
                let writes = PriorityWrite::canonical_plan(&remote.canonical_labels, desired)
                    .expect("remote Priority labels and desired outbox value are valid");
                self.checkpoint_priority_state(
                    operation.id(),
                    PriorityMutationState::Applying {
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
        self.record_priority(
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
                self.checkpoint_priority_state(
                    operation.id(),
                    PriorityMutationState::Applying {
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
                PriorityMutationState::Applied
            } else {
                PriorityMutationState::Applying {
                    expected_labels: expected_labels.clone(),
                    remaining_writes: remaining_writes.clone(),
                    last_error: None,
                }
            };
            self.checkpoint_priority_state(operation.id(), state)?;
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
        self.stage_priority_state(
            operation.id(),
            PriorityMutationState::Conflicting {
                remote: observed.clone(),
            },
        )?;
        self.record_priority(
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
        self.record_priority(
            operation,
            classification,
            Outcome::Checkpointed,
            Some(observed),
            Vec::new(),
            None,
        );
        Ok(())
    }

    fn record_missing_priority_issue(
        &mut self,
        operation: &PendingMutation,
    ) -> Result<(), ReconciliationError> {
        let error = format!("Issue #{} is absent from GitHub", operation.issue_number());
        self.stage_priority_state(
            operation.id(),
            PriorityMutationState::Failed {
                error: error.clone(),
            },
        )?;
        self.record_priority(
            operation,
            Classification::Applicable,
            Outcome::Failed,
            None,
            Vec::new(),
            Some(error),
        );
        Ok(())
    }

    fn checkpoint_priority_state(
        &mut self,
        operation_id: &str,
        state: PriorityMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .checkpoint_priority_state(self.repository, operation_id, state)?;
        Ok(())
    }

    fn stage_priority_state(
        &mut self,
        operation_id: &str,
        state: PriorityMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction.stage_priority_state(operation_id, state)?;
        Ok(())
    }

    fn observed_priority(&self, issue_number: u64) -> Option<LogicalPriority> {
        self.remote
            .get(&issue_number)
            .map(|priority| priority.logical.clone())
    }

    fn record_priority(
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

pub(super) fn verify_terminal(
    operation: &PendingMutation,
    final_priorities: &BTreeMap<u64, RemotePriority>,
    results: &mut [OperationResult],
    state_updates: &mut BTreeMap<String, MutationStateUpdate>,
) -> bool {
    let (_, desired) = operation
        .priority_values()
        .expect("Priority verifier receives a Priority mutation");
    let observed = final_priorities
        .get(&operation.issue_number())
        .map(|priority| priority.logical.clone());
    if observed.as_ref() == Some(desired) {
        true
    } else {
        convert_to_late_conflict(results, operation.id(), observed, state_updates);
        false
    }
}

pub(super) fn remote_priorities(replica: &LocalReplica) -> BTreeMap<u64, RemotePriority> {
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

fn convert_to_late_conflict(
    results: &mut [OperationResult],
    operation_id: &str,
    observed: Option<LogicalPriority>,
    state_updates: &mut BTreeMap<String, MutationStateUpdate>,
) {
    let observed = observed.unwrap_or(LogicalPriority::Unspecified);
    state_updates.insert(
        operation_id.to_owned(),
        MutationStateUpdate::Priority(PriorityMutationState::Conflicting {
            remote: observed.clone(),
        }),
    );
    if let Some(result) = results.iter_mut().find(|result| result.id == operation_id) {
        result.classification = Classification::Conflicting;
        result.outcome = Outcome::Conflicting;
        if let OperationDetails::PriorityUpdate { remote, .. } = &mut result.details {
            *remote = Some(observed);
        }
        result.error = None;
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
    let (base, desired) = operation
        .priority_values()
        .expect("Priority result is built from a Priority mutation");
    OperationResult {
        id: operation.id().to_owned(),
        issue_number: (operation.issue_number() < (1_u64 << 63))
            .then_some(operation.issue_number()),
        temporary_id: operation.priority_temporary_id(),
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        details: OperationDetails::PriorityUpdate {
            base: base.clone(),
            local: desired.clone(),
            remote,
        },
        blocked_by,
        error,
    }
}
