use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{model::TemporaryIssueId, priority::DeclaredPriority};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub(crate) struct GenericLabel(String);

impl GenericLabel {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self, MetadataError> {
        let value = value.into();
        let label = Self(value);
        label.validate()?;
        Ok(label)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn matches(&self, candidate: &str) -> bool {
        self.0.eq_ignore_ascii_case(candidate)
    }

    pub(crate) fn validate(&self) -> Result<(), MetadataError> {
        if self.0.trim().is_empty() || self.0.chars().count() > 50 {
            return Err(MetadataError::InvalidLabel);
        }
        if DeclaredPriority::parse(&self.0).is_some() {
            return Err(MetadataError::PriorityLabel);
        }
        Ok(())
    }
}

impl PartialEq for GenericLabel {
    fn eq(&self, other: &Self) -> bool {
        self.matches(other.as_str())
    }
}

impl Eq for GenericLabel {}

impl PartialOrd for GenericLabel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GenericLabel {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .to_ascii_lowercase()
            .cmp(&other.0.to_ascii_lowercase())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(try_from = "PendingIssueOperandData", into = "PendingIssueOperandData")]
pub(crate) struct PendingIssueOperand {
    number: u64,
    temporary_id: Option<TemporaryIssueId>,
}

#[derive(Deserialize, Serialize)]
struct PendingIssueOperandData {
    number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    temporary_id: Option<TemporaryIssueId>,
}

impl TryFrom<PendingIssueOperandData> for PendingIssueOperand {
    type Error = MetadataError;

    fn try_from(data: PendingIssueOperandData) -> Result<Self, Self::Error> {
        Self::new(data.number, data.temporary_id)
    }
}

impl From<PendingIssueOperand> for PendingIssueOperandData {
    fn from(operand: PendingIssueOperand) -> Self {
        Self {
            number: operand.number,
            temporary_id: operand.temporary_id,
        }
    }
}

impl PartialEq for PendingIssueOperand {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PendingIssueOperand {}

impl PartialOrd for PendingIssueOperand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingIssueOperand {
    fn cmp(&self, other: &Self) -> Ordering {
        operand_key(*self).cmp(&operand_key(*other))
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum OperandKey {
    GitHub(u64),
    Draft(TemporaryIssueId),
}

fn operand_key(operand: PendingIssueOperand) -> OperandKey {
    if operand.number < (1_u64 << 63) {
        OperandKey::GitHub(operand.number)
    } else {
        operand
            .temporary_id
            .map(OperandKey::Draft)
            .expect("Pending Issue operands are validated when constructed or deserialized")
    }
}

impl PendingIssueOperand {
    pub(crate) fn new(
        number: u64,
        temporary_id: Option<TemporaryIssueId>,
    ) -> Result<Self, MetadataError> {
        let operand = Self {
            number,
            temporary_id,
        };
        if !operand.is_valid() {
            return Err(MetadataError::InvalidIssue);
        }
        Ok(operand)
    }

    pub(crate) fn number(self) -> u64 {
        self.number
    }

    pub(crate) fn temporary_id(self) -> Option<TemporaryIssueId> {
        self.temporary_id
    }

    pub(crate) fn resolve(&mut self, temporary_id: TemporaryIssueId, issue_number: u64) {
        if self.temporary_id == Some(temporary_id) {
            self.number = issue_number;
        }
    }

    pub(crate) fn is_valid(self) -> bool {
        self.number > 0
            && match self.temporary_id {
                None => self.number < (1_u64 << 63),
                Some(temporary_id) => {
                    self.number == temporary_id.synthetic_number() || self.number < (1_u64 << 63)
                }
            }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum MetadataSetTarget {
    GenericLabel {
        issue: PendingIssueOperand,
        label: GenericLabel,
    },
    ParentRelationship {
        parent: PendingIssueOperand,
        child: PendingIssueOperand,
    },
}

impl MetadataSetTarget {
    pub(crate) fn primary_number(&self) -> u64 {
        match self {
            Self::GenericLabel { issue, .. } => issue.number(),
            Self::ParentRelationship { parent, .. } => parent.number(),
        }
    }

    pub(crate) fn affected_issue_numbers(&self) -> Vec<u64> {
        match self {
            Self::GenericLabel { issue, .. } => vec![issue.number()],
            Self::ParentRelationship { parent, child } => {
                vec![parent.number(), child.number()]
            }
        }
    }

    pub(crate) fn is_remote_resolved(&self) -> bool {
        self.affected_issue_numbers()
            .into_iter()
            .all(|number| number < (1_u64 << 63))
    }

    pub(crate) fn resolve(&mut self, temporary_id: TemporaryIssueId, issue_number: u64) {
        match self {
            Self::GenericLabel { issue, .. } => issue.resolve(temporary_id, issue_number),
            Self::ParentRelationship { parent, child } => {
                parent.resolve(temporary_id, issue_number);
                child.resolve(temporary_id, issue_number);
            }
        }
    }

    pub(crate) fn validate(&self) -> Result<(), MetadataError> {
        match self {
            Self::GenericLabel { issue, label } => {
                if !issue.is_valid() {
                    return Err(MetadataError::InvalidIssue);
                }
                label.validate()
            }
            Self::ParentRelationship { parent, child } => {
                if !parent.is_valid() || !child.is_valid() || parent.number() == child.number() {
                    return Err(MetadataError::InvalidRelationship);
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum MetadataError {
    #[error("generic label names must contain 1 to 50 characters")]
    InvalidLabel,
    #[error("canonical Priority labels must be changed with `hyfa update ISSUE --priority`")]
    PriorityLabel,
    #[error("metadata mutation references an invalid Issue identity")]
    InvalidIssue,
    #[error("a parent and sub-Issue must be distinct valid Issues")]
    InvalidRelationship,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn set_targets_use_githubs_case_insensitive_labels_and_stable_mapped_identity() {
        let temporary_id = TemporaryIssueId::from_str("12345678-1234-1234-1234-123456789abc")
            .expect("Temporary ID");
        assert_eq!(
            GenericLabel::new("area:Core").unwrap(),
            GenericLabel::new("AREA:core").unwrap()
        );
        assert_eq!(
            PendingIssueOperand::new(42, Some(temporary_id)).unwrap(),
            PendingIssueOperand::new(42, None).unwrap(),
            "a retained Draft alias and its canonical Issue identify one set target"
        );
    }
}
