use super::*;

impl ReconciliationPass<'_, '_, '_> {
    pub(super) fn reconcile_dependency_operation(
        &mut self,
        operation: &PendingMutation,
        edge: DependencyEdgeKey,
        desired: DependencyPresence,
        blocked_by: Vec<String>,
    ) -> Result<(), ReconciliationError> {
        if !blocked_by.is_empty() {
            self.stage_dependency_state(
                operation.id(),
                DependencyMutationState::TransitivelyBlocked {
                    blocked_by: blocked_by.clone(),
                },
            )?;
            self.record_dependency(
                operation,
                Classification::TransitivelyBlocked,
                Outcome::TransitivelyBlocked,
                Some(self.remote_dependencies.contains(&edge)),
                blocked_by,
                None,
            );
            return Ok(());
        }

        if !self.remote.contains_key(&edge.blocked_number()) {
            return self.record_missing_dependency_issue(operation, edge.blocked_number());
        }
        if edge.is_internal() && !self.remote.contains_key(&edge.blocker_number()) {
            return self.record_missing_dependency_issue(operation, edge.blocker_number());
        }
        let present = self.remote_dependencies.contains(&edge);
        let state = self
            .transaction
            .operation(operation.id())
            .expect("known operation")
            .dependency_state()
            .expect("Dependency operation has Dependency state")
            .clone();
        match state {
            DependencyMutationState::Applied => {
                self.record_dependency_terminal(operation, Classification::Applicable, present)
            }
            DependencyMutationState::AlreadySatisfied => self.record_dependency_terminal(
                operation,
                Classification::AlreadySatisfied,
                present,
            ),
            DependencyMutationState::Pending
            | DependencyMutationState::Applying { .. }
            | DependencyMutationState::Failed { .. }
            | DependencyMutationState::TransitivelyBlocked { .. } => {
                if present == desired.is_present() {
                    self.stage_dependency_state(
                        operation.id(),
                        DependencyMutationState::AlreadySatisfied,
                    )?;
                    self.record_dependency(
                        operation,
                        Classification::AlreadySatisfied,
                        Outcome::AlreadySatisfied,
                        Some(present),
                        Vec::new(),
                        None,
                    );
                    Ok(())
                } else {
                    self.apply_dependency(operation, edge, desired, present)
                }
            }
        }
    }

    fn apply_dependency(
        &mut self,
        operation: &PendingMutation,
        edge: DependencyEdgeKey,
        desired: DependencyPresence,
        observed_present: bool,
    ) -> Result<(), ReconciliationError> {
        self.checkpoint_dependency_state(
            operation.id(),
            DependencyMutationState::Applying { last_error: None },
        )?;
        self.requires_final_refresh = true;
        let blocked = issue_reference(edge.blocked_repository(), edge.blocked_number());
        let blocker = issue_reference(edge.blocker_repository(), edge.blocker_number());
        let intent = if desired.is_present() {
            DependencyIntent::Block
        } else {
            DependencyIntent::Unblock
        };
        match self.client.mutate_dependency(&blocked, &blocker, intent) {
            Ok(_) => {
                if desired.is_present() {
                    self.remote_dependencies.insert(edge);
                } else {
                    self.remote_dependencies.remove(&edge);
                }
                self.checkpoint_dependency_state(operation.id(), DependencyMutationState::Applied)?;
                self.record_dependency(
                    operation,
                    Classification::Applicable,
                    Outcome::Applied,
                    Some(desired.is_present()),
                    Vec::new(),
                    None,
                );
            }
            Err(error) => {
                let message = error.to_string();
                self.checkpoint_dependency_state(
                    operation.id(),
                    DependencyMutationState::Applying {
                        last_error: Some(message.clone()),
                    },
                )?;
                self.record_dependency(
                    operation,
                    Classification::Applicable,
                    Outcome::Failed,
                    Some(observed_present),
                    Vec::new(),
                    Some(message),
                );
            }
        }
        Ok(())
    }

    fn record_missing_dependency_issue(
        &mut self,
        operation: &PendingMutation,
        issue_number: u64,
    ) -> Result<(), ReconciliationError> {
        let error = format!("Issue #{issue_number} is absent from GitHub");
        self.stage_dependency_state(
            operation.id(),
            DependencyMutationState::Failed {
                error: error.clone(),
            },
        )?;
        self.record_dependency(
            operation,
            Classification::Applicable,
            Outcome::Failed,
            None,
            Vec::new(),
            Some(error),
        );
        Ok(())
    }

    fn record_dependency_terminal(
        &mut self,
        operation: &PendingMutation,
        classification: Classification,
        present: bool,
    ) -> Result<(), ReconciliationError> {
        self.record_dependency(
            operation,
            classification,
            Outcome::Checkpointed,
            Some(present),
            Vec::new(),
            None,
        );
        Ok(())
    }

    fn checkpoint_dependency_state(
        &mut self,
        operation_id: &str,
        state: DependencyMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .checkpoint_dependency_state(self.repository, operation_id, state)?;
        Ok(())
    }

    fn stage_dependency_state(
        &mut self,
        operation_id: &str,
        state: DependencyMutationState,
    ) -> Result<(), ReconciliationError> {
        self.transaction
            .stage_dependency_state(operation_id, state)?;
        Ok(())
    }

    fn record_dependency(
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

pub(super) fn verify_terminal(
    operation: &PendingMutation,
    final_dependencies: &BTreeSet<DependencyEdgeKey>,
    results: &mut [OperationResult],
    state_updates: &mut BTreeMap<String, MutationStateUpdate>,
) -> bool {
    let (edge, desired) = operation
        .dependency_values()
        .expect("Dependency verifier receives a Dependency mutation");
    let observed = final_dependencies.contains(edge);
    if observed == desired.is_present() {
        true
    } else {
        convert_to_failed_verification(results, operation.id(), observed, state_updates);
        false
    }
}

pub(super) fn remote_dependencies(replica: &LocalReplica) -> BTreeSet<DependencyEdgeKey> {
    replica
        .dependencies
        .iter()
        .filter(|dependency| {
            dependency
                .blocked
                .repository
                .eq_ignore_ascii_case(&replica.repository)
        })
        .map(DependencyEdgeKey::from_dependency)
        .collect()
}

fn issue_reference(repository: &str, issue_number: u64) -> IssueReference {
    IssueReference::parse(&format!("{repository}#{issue_number}"))
        .expect("validated Repository and positive outbox Issue number form a valid reference")
}

fn convert_to_failed_verification(
    results: &mut [OperationResult],
    operation_id: &str,
    observed: bool,
    state_updates: &mut BTreeMap<String, MutationStateUpdate>,
) {
    let error = "final GitHub synchronization did not verify the desired Dependency state";
    state_updates.insert(
        operation_id.to_owned(),
        MutationStateUpdate::Dependency(DependencyMutationState::Applying {
            last_error: Some(error.to_owned()),
        }),
    );
    if let Some(result) = results.iter_mut().find(|result| result.id == operation_id) {
        result.classification = Classification::Applicable;
        result.outcome = Outcome::Failed;
        if let OperationDetails::DependencyUpdate { remote_present, .. } = &mut result.details {
            *remote_present = Some(observed);
        }
        result.error = Some(error.to_owned());
    }
}

fn result_for(
    operation: &PendingMutation,
    classification: Classification,
    outcome: Outcome,
    remote_present: Option<bool>,
    blocked_by: Vec<String>,
    error: Option<String>,
) -> OperationResult {
    let (edge, desired) = operation
        .dependency_values()
        .expect("Dependency result is built from a Dependency mutation");
    OperationResult {
        id: operation.id().to_owned(),
        issue_number: edge.blocked_number(),
        depends_on: operation.depends_on().to_vec(),
        classification,
        outcome,
        details: OperationDetails::DependencyUpdate {
            edge: DependencyEdgeResult {
                blocked_number: edge.blocked_number(),
                blocker_repository: edge.blocker_repository().to_owned(),
                blocker_number: edge.blocker_number(),
            },
            desired_present: desired.is_present(),
            remote_present,
        },
        blocked_by,
        error,
    }
}
