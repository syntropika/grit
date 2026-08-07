use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    model::{
        BlockerIdentity, BlockerScope, Dependency, DependencyEdgeKey, DependencyPresence, Issue,
        IssueIdentity, LocalReplica,
    },
    outbox::PendingMutationOutbox,
    priority::{LogicalPriority, PriorityState},
};

pub(crate) struct WorkingGraph<'a> {
    replica: Cow<'a, LocalReplica>,
    priority_overrides: BTreeMap<u64, LogicalPriority>,
    operation_ids: Vec<String>,
    operation_ids_by_issue: BTreeMap<u64, Vec<(usize, String)>>,
    topology_operation_ids: Vec<(usize, String)>,
    input_hash: String,
}

impl<'a> WorkingGraph<'a> {
    pub(crate) fn project(
        replica: &'a LocalReplica,
        outbox: &'a PendingMutationOutbox,
    ) -> Result<Self, WorkingGraphError> {
        Self::validate_references(replica, outbox)?;
        let mut effective_replica = Cow::Borrowed(replica);
        let mut priority_overrides = BTreeMap::new();
        let mut dependency_intents = Vec::new();
        let mut operation_ids = Vec::with_capacity(outbox.operations().len());
        let mut operation_ids_by_issue = BTreeMap::<u64, Vec<(usize, String)>>::new();
        let mut topology_operation_ids = Vec::new();
        for (index, operation) in outbox.operations().iter().enumerate() {
            let affected_issue_numbers = operation.affected_issue_numbers();
            if let Some(priority) = operation.effective_priority() {
                priority_overrides.insert(operation.issue_number(), priority.clone());
            }
            if let Some((edge, desired)) = operation.dependency_values() {
                dependency_intents.push((edge.clone(), desired));
            }
            if !operation.is_pending_intent() {
                continue;
            }
            let operation_id = operation.id().to_owned();
            operation_ids.push(operation_id.clone());
            if operation.dependency_values().is_some() {
                topology_operation_ids.push((index, operation_id.clone()));
            }
            for issue_number in affected_issue_numbers {
                operation_ids_by_issue
                    .entry(issue_number)
                    .or_default()
                    .push((index, operation_id.clone()));
            }
        }
        if !dependency_intents.is_empty() {
            project_dependency_intents(effective_replica.to_mut(), dependency_intents)?;
        }
        let input = json!({
            "schema_version": "grit.working-graph/v1",
            "replica_snapshot_hash": replica.input_hash,
            "pending_mutations": outbox.operations(),
        });
        let canonical = serde_json::to_vec(&input).map_err(WorkingGraphError::EncodeHashInput)?;
        let input_hash = hex::encode(Sha256::digest(canonical));
        Ok(Self {
            replica: effective_replica,
            priority_overrides,
            operation_ids,
            operation_ids_by_issue,
            topology_operation_ids,
            input_hash,
        })
    }

    pub(crate) fn validate_references(
        replica: &LocalReplica,
        outbox: &PendingMutationOutbox,
    ) -> Result<(), WorkingGraphError> {
        validated_issue_numbers(replica, outbox).map(drop)
    }

    pub(crate) fn replica(&self) -> &LocalReplica {
        &self.replica
    }

    pub(crate) fn input_hash(&self) -> &str {
        &self.input_hash
    }

    pub(crate) fn is_pending(&self) -> bool {
        !self.operation_ids.is_empty()
    }

    pub(crate) fn operation_ids(&self) -> Vec<String> {
        self.operation_ids.clone()
    }

    pub(crate) fn priority(&self, issue: &Issue) -> PriorityState {
        self.priority_overrides
            .get(&issue.number)
            .map(LogicalPriority::to_state)
            .unwrap_or_else(|| PriorityState::from_issue_labels(&issue.labels))
    }

    pub(crate) fn provenance_for_issue(&self, issue_number: u64) -> PendingProvenance {
        PendingProvenance::new(
            self.operation_ids_by_issue
                .get(&issue_number)
                .into_iter()
                .flatten()
                .map(|(_, operation_id)| operation_id.clone())
                .collect(),
        )
    }

    pub(crate) fn ranking_provenance_for_issues(
        &self,
        issue_numbers: impl IntoIterator<Item = u64>,
    ) -> PendingProvenance {
        let issue_numbers: BTreeSet<_> = issue_numbers.into_iter().collect();
        let mut indexed_operation_ids: Vec<_> = issue_numbers
            .iter()
            .filter_map(|issue_number| self.operation_ids_by_issue.get(issue_number))
            .flatten()
            .cloned()
            .collect();
        indexed_operation_ids.extend(self.topology_operation_ids.iter().cloned());
        PendingProvenance::from_indexed(indexed_operation_ids)
    }
}

fn validated_issue_numbers(
    replica: &LocalReplica,
    outbox: &PendingMutationOutbox,
) -> Result<BTreeSet<u64>, WorkingGraphError> {
    let issue_numbers: BTreeSet<_> = replica.issues.iter().map(|issue| issue.number).collect();
    for operation in outbox.operations() {
        for issue_number in operation.affected_issue_numbers() {
            if !issue_numbers.contains(&issue_number) {
                return Err(WorkingGraphError::MissingIssue(issue_number));
            }
        }
    }
    Ok(issue_numbers)
}

fn project_dependency_intents(
    replica: &mut LocalReplica,
    intents: Vec<(DependencyEdgeKey, DependencyPresence)>,
) -> Result<(), WorkingGraphError> {
    let issues: BTreeMap<_, _> = replica
        .issues
        .iter()
        .map(|issue| (issue.number, issue))
        .collect();
    let mut dependencies: BTreeMap<DependencyEdgeKey, Dependency> =
        std::mem::take(&mut replica.dependencies)
            .into_iter()
            .map(|dependency| (DependencyEdgeKey::from_dependency(&dependency), dependency))
            .collect();
    for (edge, desired) in intents {
        let normalized = edge.clone();
        match desired {
            DependencyPresence::Present if !dependencies.contains_key(&normalized) => {
                let blocked = issues
                    .get(&edge.blocked_number())
                    .copied()
                    .ok_or(WorkingGraphError::MissingIssue(edge.blocked_number()))?;
                let internal = edge.is_internal();
                let blocker = internal
                    .then(|| {
                        issues
                            .get(&edge.blocker_number())
                            .copied()
                            .ok_or(WorkingGraphError::MissingIssue(edge.blocker_number()))
                    })
                    .transpose()?;
                dependencies.insert(
                    normalized,
                    Dependency {
                        blocked: IssueIdentity {
                            repository: replica.repository.clone(),
                            number: blocked.number,
                            id: blocked.id,
                            node_id: blocked.node_id.clone(),
                        },
                        blocker: BlockerIdentity {
                            repository: edge.blocker_repository().to_owned(),
                            number: edge.blocker_number(),
                            state: blocker
                                .map(|issue| issue.state.clone())
                                .unwrap_or_else(|| "unknown".to_owned()),
                            scope: if internal {
                                BlockerScope::Internal
                            } else {
                                BlockerScope::External
                            },
                            id: blocker.map(|issue| issue.id),
                            node_id: blocker.map(|issue| issue.node_id.clone()),
                        },
                    },
                );
            }
            DependencyPresence::Absent => {
                dependencies.remove(&normalized);
            }
            DependencyPresence::Present => {}
        }
    }
    replica.dependencies = dependencies.into_values().collect();
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PendingProvenance {
    pending: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    operation_ids: Vec<String>,
}

impl PendingProvenance {
    pub(crate) fn new(operation_ids: Vec<String>) -> Self {
        Self {
            pending: !operation_ids.is_empty(),
            operation_ids,
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }

    fn from_indexed(mut indexed_operation_ids: Vec<(usize, String)>) -> Self {
        indexed_operation_ids.sort_by_key(|(index, _)| *index);
        indexed_operation_ids.dedup_by(|left, right| left.1 == right.1);
        Self::new(
            indexed_operation_ids
                .into_iter()
                .map(|(_, operation_id)| operation_id)
                .collect(),
        )
    }
}

#[derive(Debug, Error)]
pub(crate) enum WorkingGraphError {
    #[error("could not encode the Working graph input for hashing: {0}")]
    EncodeHashInput(serde_json::Error),
    #[error("Pending mutations reference Issue #{0}, which is absent from the Local replica")]
    MissingIssue(u64),
}
