use serde::Serialize;
use thiserror::Error;

use crate::{
    model::{IssueRelationships, TemporaryIssueId, strip_operation_markers},
    operational::{
        PreparedRepository,
        impact::{DependencyImpact, ImpactAnalysis},
    },
    priority::PriorityState,
    working_graph::{PendingProvenance, WorkingGraph},
};

#[derive(Serialize)]
pub(crate) struct IssueView {
    pub(crate) key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temporary_id: Option<TemporaryIssueId>,
    url: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) state: String,
    state_reason: Option<String>,
    author: Option<String>,
    pub(crate) assignees: Vec<String>,
    pub(crate) labels: Vec<String>,
    priority: PriorityState,
    pub(crate) comments: Vec<CommentView>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    #[serde(flatten)]
    provenance: PendingProvenance,
    pub(crate) relationships_complete: bool,
    pub(crate) relationships: Option<IssueRelationships>,
    pub(crate) blocked_by: Vec<BlockerView>,
    pub(crate) blocks: Vec<String>,
    pub(crate) impact: Option<DependencyImpact>,
}

#[derive(Serialize)]
pub(crate) struct CommentView {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    url: String,
    pub(crate) author: Option<String>,
    pub(crate) body: String,
    pub(crate) created_at: String,
    updated_at: String,
    author_association: String,
    pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct BlockerView {
    pub(crate) key: String,
    pub(crate) state: String,
    external: bool,
    #[serde(flatten)]
    provenance: PendingProvenance,
}

pub(crate) fn read(working: &WorkingGraph<'_>, number: u64) -> Result<IssueView, IssueReadError> {
    let replica = working.replica();
    let issue = replica
        .issues
        .iter()
        .find(|issue| issue.number == number)
        .ok_or(IssueReadError::MissingIssue)?;
    let key = issue.display_key(&replica.repository);
    let related_key = |repository: &str, number: u64| {
        if repository.eq_ignore_ascii_case(&replica.repository) {
            replica
                .issues
                .iter()
                .find(|issue| issue.number == number)
                .map(|issue| issue.display_key(&replica.repository))
                .ok_or(IssueReadError::MissingRelationshipEndpoint)
        } else {
            Ok(format!("{repository}#{number}"))
        }
    };
    let blocked_by = replica
        .dependencies
        .iter()
        .filter(|edge| edge.blocked.number == number)
        .map(|edge| {
            let blocker = &edge.blocker;
            let external = !blocker.repository.eq_ignore_ascii_case(&replica.repository);
            let state = if external {
                blocker.state.clone()
            } else {
                replica
                    .issues
                    .iter()
                    .find(|issue| issue.number == blocker.number)
                    .map(|issue| issue.state.clone())
                    .unwrap_or_else(|| "unknown".to_owned())
            };
            Ok(BlockerView {
                key: related_key(&blocker.repository, blocker.number)?,
                state,
                external,
                provenance: working.provenance_for_dependency(
                    &crate::model::DependencyEdgeKey::from_dependency(edge),
                ),
            })
        })
        .collect::<Result<Vec<_>, IssueReadError>>()?;
    let blocks = replica
        .dependencies
        .iter()
        .filter(|edge| {
            edge.blocker.number == number
                && edge
                    .blocker
                    .repository
                    .eq_ignore_ascii_case(&replica.repository)
        })
        .map(|edge| related_key(&edge.blocked.repository, edge.blocked.number))
        .collect::<Result<Vec<_>, _>>()?;
    let relationships = replica
        .relationships
        .get(&key.to_ascii_lowercase())
        .cloned();
    let prepared = PreparedRepository::prepare(working);
    let impact = ImpactAnalysis::prepare(&prepared).for_issue(number);
    Ok(IssueView {
        key,
        number: (!issue.is_draft()).then_some(issue.number),
        temporary_id: issue.temporary_id(),
        url: issue.url.clone(),
        title: strip_operation_markers(&issue.title),
        body: strip_operation_markers(&issue.body),
        state: issue.state.clone(),
        state_reason: issue.state_reason.clone(),
        author: issue.author.as_ref().map(|author| author.login.clone()),
        assignees: issue
            .assignees
            .iter()
            .map(|actor| actor.login.clone())
            .collect(),
        labels: issue
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect(),
        priority: working.priority(issue),
        comments: issue
            .comments
            .iter()
            .map(|comment| {
                let operation_id = comment.node_id.strip_prefix("pending:").map(str::to_owned);
                CommentView {
                    id: (comment.id != 0).then_some(comment.id),
                    url: comment.url.clone(),
                    author: comment.author.as_ref().map(|author| author.login.clone()),
                    body: strip_operation_markers(&comment.body),
                    created_at: comment.created_at.clone(),
                    updated_at: comment.updated_at.clone(),
                    author_association: comment.author_association.clone(),
                    pending: operation_id.is_some(),
                    operation_id,
                }
            })
            .collect(),
        created_at: issue.created_at.clone(),
        updated_at: issue.updated_at.clone(),
        closed_at: issue.closed_at.clone(),
        provenance: working.provenance_for_issue(number),
        relationships_complete: relationships.is_some(),
        relationships,
        blocked_by,
        blocks,
        impact,
    })
}

#[derive(Debug, Error)]
pub(crate) enum IssueReadError {
    #[error("the requested Issue is absent from the effective local view")]
    MissingIssue,
    #[error("the Issue has a relationship endpoint missing from the effective local view")]
    MissingRelationshipEndpoint,
}
