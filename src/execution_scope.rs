use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{model::Issue, repository::PendingIssueReference, working_graph::WorkingGraph};

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) exclude_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) children_of: Option<String>,
    #[serde(skip)]
    children: Option<BTreeSet<u64>>,
}

impl Selection {
    pub(crate) fn new(
        labels: &[String],
        exclude_labels: &[String],
        children_of: Option<String>,
        working: &WorkingGraph<'_>,
    ) -> Result<Self, SelectionError> {
        let normalize = |labels: &[String]| -> Result<Vec<String>, SelectionError> {
            let mut labels: Vec<_> = labels
                .iter()
                .map(|label| label.to_ascii_lowercase())
                .collect();
            if labels
                .iter()
                .any(|label| label.trim().is_empty() || label.chars().any(char::is_control))
            {
                return Err(SelectionError::InvalidLabel);
            }
            labels.sort();
            labels.dedup();
            Ok(labels)
        };
        let children = children_of
            .as_ref()
            .map(|parent| -> Result<BTreeSet<u64>, SelectionError> {
                let inventory = working
                    .replica()
                    .relationships
                    .get(parent)
                    .ok_or_else(|| SelectionError::MissingRelationships(parent.clone()))?;
                let keys: BTreeSet<_> = inventory.children.iter().collect();
                Ok(working
                    .replica()
                    .issues
                    .iter()
                    .filter(|issue| {
                        keys.contains(
                            &issue
                                .display_key(&working.replica().repository)
                                .to_ascii_lowercase(),
                        )
                    })
                    .map(|issue| issue.number)
                    .collect())
            })
            .transpose()?;
        Ok(Self {
            labels: normalize(labels)?,
            exclude_labels: normalize(exclude_labels)?,
            children_of,
            children,
        })
    }

    pub(crate) fn contains(&self, issue: &Issue) -> bool {
        let has = |name: &str| {
            issue
                .labels
                .iter()
                .any(|label| label.name.eq_ignore_ascii_case(name))
        };
        self.labels.iter().all(|label| has(label))
            && !self.exclude_labels.iter().any(|label| has(label))
            && self
                .children
                .as_ref()
                .is_none_or(|children| children.contains(&issue.number))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.exclude_labels.is_empty() && self.children_of.is_none()
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ScopeDescription {
    Available {
        #[serde(flatten)]
        selection: Selection,
    },
    Assignee {
        assignee: String,
        #[serde(flatten)]
        selection: Selection,
    },
}

#[derive(Clone, Copy, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EmptyReason {
    NoOpenIssues,
    NoReadyIssues,
    ReadyIssuesOutsideScope,
}

pub(crate) fn empty_reason(ready: &crate::operational::ReadyAnalysis<'_>) -> Option<EmptyReason> {
    if !ready.executable.is_empty() {
        None
    } else if ready.operational_issue_count == 0 {
        Some(EmptyReason::NoOpenIssues)
    } else if ready.ready_count == 0 {
        Some(EmptyReason::NoReadyIssues)
    } else {
        Some(EmptyReason::ReadyIssuesOutsideScope)
    }
}

pub(crate) fn parent_key(value: &str, repository: &str) -> Result<String, SelectionError> {
    let reference =
        PendingIssueReference::parse(value).map_err(|_| SelectionError::InvalidParent)?;
    if !reference
        .repository()
        .full_name()
        .eq_ignore_ascii_case(repository)
    {
        return Err(SelectionError::ForeignParent);
    }
    Ok(crate::draft_identity::resolve_reference(&reference)?
        .stable_key()
        .to_ascii_lowercase())
}

#[derive(Debug, Error)]
pub(crate) enum SelectionError {
    #[error(transparent)]
    DraftIdentity(#[from] crate::draft_identity::DraftIdentityError),
    #[error("selection labels must be nonempty and contain no control characters")]
    InvalidLabel,
    #[error("children-of must be an OWNER/REPO#NUMBER or draft reference")]
    InvalidParent,
    #[error("the selected parent must belong to the analysis Repository")]
    ForeignParent,
    #[error(
        "no complete relationship inventory for {0}; run this command online before using its parent scope offline"
    )]
    MissingRelationships(String),
}
