use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    model::{Issue, LocalReplica},
    outbox::PendingMutationOutbox,
    priority::{LogicalPriority, PriorityState},
};

pub(crate) struct WorkingGraph<'a> {
    replica: &'a LocalReplica,
    priority_overrides: BTreeMap<u64, LogicalPriority>,
    operation_ids: Vec<String>,
    operation_ids_by_issue: BTreeMap<u64, Vec<(usize, String)>>,
    input_hash: String,
}

impl<'a> WorkingGraph<'a> {
    pub(crate) fn project(
        replica: &'a LocalReplica,
        outbox: &'a PendingMutationOutbox,
    ) -> Result<Self, WorkingGraphError> {
        let issue_numbers: BTreeSet<_> = replica.issues.iter().map(|issue| issue.number).collect();
        let mut priority_overrides = BTreeMap::new();
        let mut operation_ids = Vec::with_capacity(outbox.operations().len());
        let mut operation_ids_by_issue = BTreeMap::<u64, Vec<(usize, String)>>::new();
        for (index, operation) in outbox.operations().iter().enumerate() {
            let issue_number = operation.issue_number();
            if !issue_numbers.contains(&issue_number) {
                return Err(WorkingGraphError::MissingIssue(issue_number));
            }
            priority_overrides.insert(issue_number, operation.desired().clone());
            let operation_id = operation.id().to_owned();
            operation_ids.push(operation_id.clone());
            operation_ids_by_issue
                .entry(issue_number)
                .or_default()
                .push((index, operation_id));
        }
        let input = json!({
            "schema_version": "grit.working-graph/v1",
            "replica_snapshot_hash": replica.input_hash,
            "pending_mutations": outbox.operations(),
        });
        let canonical = serde_json::to_vec(&input).map_err(WorkingGraphError::EncodeHashInput)?;
        let input_hash = hex::encode(Sha256::digest(canonical));
        Ok(Self {
            replica,
            priority_overrides,
            operation_ids,
            operation_ids_by_issue,
            input_hash,
        })
    }

    pub(crate) fn replica(&self) -> &'a LocalReplica {
        self.replica
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

    pub(crate) fn priority_is_pending(&self, issue_number: u64) -> bool {
        self.priority_overrides.contains_key(&issue_number)
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

    pub(crate) fn provenance_for_issues(
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
        indexed_operation_ids.sort_by_key(|(index, _)| *index);
        let operation_ids = indexed_operation_ids
            .into_iter()
            .map(|(_, operation_id)| operation_id)
            .collect();
        PendingProvenance::new(operation_ids)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingProvenance {
    pending: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

    pub(crate) fn operation_ids(&self) -> &[String] {
        &self.operation_ids
    }
}

#[derive(Debug, Error)]
pub(crate) enum WorkingGraphError {
    #[error("could not encode the Working graph input for hashing: {0}")]
    EncodeHashInput(serde_json::Error),
    #[error("Pending mutations reference Issue #{0}, which is absent from the Local replica")]
    MissingIssue(u64),
}
