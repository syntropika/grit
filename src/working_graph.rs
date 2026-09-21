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
        IssueIdentity, Label, LocalReplica, SetPresence,
    },
    outbox::{IssueCreateState, PendingMutation, PendingMutationOutbox},
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
        let mut field_updates = Vec::new();
        let mut label_updates = Vec::new();
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
            if let Some((issue_number, field, value)) = operation.effective_field_update() {
                field_updates.push((issue_number, field, value.clone()));
            }
            if let Some((
                crate::metadata::MetadataSetTarget::GenericLabel { issue, label },
                desired,
            )) = operation.metadata_set_values()
            {
                label_updates.push((issue.number(), label.clone(), desired));
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
        let synchronized_numbers: BTreeSet<_> =
            replica.issues.iter().map(|issue| issue.number).collect();
        let draft_issues: Vec<_> = outbox
            .operations()
            .iter()
            .filter_map(draft_issue)
            .filter(|issue| !synchronized_numbers.contains(&issue.number))
            .collect();
        if !draft_issues.is_empty() {
            effective_replica.to_mut().issues.extend(draft_issues);
            effective_replica
                .to_mut()
                .issues
                .sort_by_key(|issue| issue.stable_node_key());
        }
        if !field_updates.is_empty() {
            apply_field_updates(&mut effective_replica.to_mut().issues, field_updates)?;
        }
        if !label_updates.is_empty() {
            apply_label_updates(&mut effective_replica.to_mut().issues, label_updates)?;
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

fn apply_label_updates(
    issues: &mut [Issue],
    updates: Vec<(u64, crate::metadata::GenericLabel, SetPresence)>,
) -> Result<(), WorkingGraphError> {
    let issue_indices: BTreeMap<_, _> = issues
        .iter()
        .enumerate()
        .map(|(index, issue)| (issue.number, index))
        .collect();
    for (issue_number, label, desired) in updates {
        let index = issue_indices
            .get(&issue_number)
            .copied()
            .ok_or(WorkingGraphError::MissingIssue(issue_number))?;
        let labels = &mut issues[index].labels;
        let existing = labels
            .iter()
            .position(|candidate| label.matches(&candidate.name));
        match (desired, existing) {
            (SetPresence::Present, None) => labels.push(Label {
                id: None,
                node_id: None,
                name: label.as_str().to_owned(),
                color: None,
                description: None,
            }),
            (SetPresence::Absent, Some(index)) => {
                labels.remove(index);
            }
            (SetPresence::Present, Some(_)) | (SetPresence::Absent, None) => {}
        }
        labels.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
    }
    Ok(())
}

fn apply_field_updates(
    issues: &mut [Issue],
    updates: Vec<(
        u64,
        crate::issue_field::IssueField,
        crate::issue_field::IssueFieldValue,
    )>,
) -> Result<(), WorkingGraphError> {
    let issue_indices: BTreeMap<_, _> = issues
        .iter()
        .enumerate()
        .map(|(index, issue)| (issue.number, index))
        .collect();
    for (issue_number, field, value) in updates {
        let index = issue_indices
            .get(&issue_number)
            .copied()
            .ok_or(WorkingGraphError::MissingIssue(issue_number))?;
        debug_assert_eq!(value.field(), field);
        value.apply_to(&mut issues[index]);
    }
    Ok(())
}

fn validated_issue_numbers(
    replica: &LocalReplica,
    outbox: &PendingMutationOutbox,
) -> Result<BTreeSet<u64>, WorkingGraphError> {
    let mut issue_numbers: BTreeSet<_> = replica.issues.iter().map(|issue| issue.number).collect();
    issue_numbers.extend(outbox.operations().iter().filter_map(|operation| {
        operation
            .issue_create_view()
            .map(|_| operation.issue_number())
    }));
    for operation in outbox.operations() {
        for issue_number in operation.affected_issue_numbers() {
            if !issue_numbers.contains(&issue_number) {
                return Err(WorkingGraphError::MissingIssue(issue_number));
            }
        }
    }
    Ok(issue_numbers)
}

fn draft_issue(operation: &PendingMutation) -> Option<Issue> {
    let create = operation.issue_create_view()?;
    let (id, node_id, number, url, identity) = match create.state {
        IssueCreateState::Mapped {
            issue_id,
            issue_node_id,
            issue_number,
            issue_url,
        } => (
            *issue_id,
            issue_node_id.clone(),
            *issue_number,
            issue_url.clone(),
            crate::model::IssueIdentityState::MappedDraft(create.temporary_id),
        ),
        _ => (
            0,
            format!("draft:{}", create.temporary_id),
            create.synthetic_number,
            String::new(),
            crate::model::IssueIdentityState::Draft(create.temporary_id),
        ),
    };
    Some(Issue {
        id,
        node_id,
        number,
        url,
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
        identity,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        issue_field::{IssueField, IssueFieldValue},
        model::IssueIdentityState,
    };

    #[test]
    fn field_projection_scales_by_indexing_issues_once_and_preserves_operation_order() {
        let mut issues: Vec<_> = (1..=5_000).map(issue).collect();
        let mut updates = Vec::new();
        for number in 1..=5_000 {
            updates.push((
                number,
                IssueField::Title,
                IssueFieldValue::title(format!("first-{number}")),
            ));
            updates.push((
                number,
                IssueField::Title,
                IssueFieldValue::title(format!("last-{number}")),
            ));
        }

        apply_field_updates(&mut issues, updates).expect("project field updates");

        assert_eq!(issues[0].title, "last-1");
        assert_eq!(issues[4_999].title, "last-5000");
    }

    fn issue(number: u64) -> Issue {
        Issue {
            id: number,
            node_id: format!("I_{number}"),
            number,
            url: format!("https://github.com/acme/widgets/issues/{number}"),
            title: format!("Issue {number}"),
            body: String::new(),
            state: "open".to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            updated_at: "2026-08-07T00:00:00Z".to_owned(),
            closed_at: None,
            identity: IssueIdentityState::GitHub,
        }
    }
}
