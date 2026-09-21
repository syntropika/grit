use std::{fmt, str::FromStr};

use serde::{Serialize, Serializer};

use crate::model::Label;

pub(crate) struct PriorityLabelSpec {
    pub(crate) name: &'static str,
    pub(crate) color: &'static str,
    pub(crate) description: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeclaredPriority {
    P0,
    P1,
    P2,
    P3,
    P4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrioritySelection {
    Declared(DeclaredPriority),
    None,
}

impl PrioritySelection {
    pub(crate) fn desired(self) -> Option<DeclaredPriority> {
        match self {
            Self::Declared(priority) => Some(priority),
            Self::None => None,
        }
    }

    pub(crate) fn logical_name(self) -> &'static str {
        self.desired()
            .map(DeclaredPriority::canonical_label)
            .and_then(|label| label.strip_prefix("priority:"))
            .unwrap_or("unspecified")
    }
}

impl FromStr for PrioritySelection {
    type Err = PrioritySelectionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        DeclaredPriority::parse(&format!("priority:{value}"))
            .map(Self::Declared)
            .ok_or(PrioritySelectionError)
    }
}

#[derive(Debug)]
pub(crate) struct PrioritySelectionError;

impl fmt::Display for PrioritySelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected p0, p1, p2, p3, p4, or none")
    }
}

impl std::error::Error for PrioritySelectionError {}

impl DeclaredPriority {
    pub(crate) const ALL: [Self; 5] = [Self::P0, Self::P1, Self::P2, Self::P3, Self::P4];

    pub(crate) fn spec(self) -> PriorityLabelSpec {
        match self {
            Self::P0 => PriorityLabelSpec {
                name: "priority:p0",
                color: "b60205",
                description: "Critical priority",
            },
            Self::P1 => PriorityLabelSpec {
                name: "priority:p1",
                color: "d93f0b",
                description: "High priority",
            },
            Self::P2 => PriorityLabelSpec {
                name: "priority:p2",
                color: "fbca04",
                description: "Normal priority",
            },
            Self::P3 => PriorityLabelSpec {
                name: "priority:p3",
                color: "0e8a16",
                description: "Low priority",
            },
            Self::P4 => PriorityLabelSpec {
                name: "priority:p4",
                color: "6a737d",
                description: "Lowest priority",
            },
        }
    }

    pub(crate) fn parse(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|priority| label.eq_ignore_ascii_case(priority.spec().name))
    }

    pub(crate) fn canonical_label(self) -> &'static str {
        self.spec().name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PriorityComparison {
    P0,
    P1,
    Neutral,
    P3,
    P4,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PriorityState {
    Declared { value: DeclaredPriority },
    Unspecified,
    Conflict { labels: Vec<String> },
}

impl PriorityState {
    pub(crate) fn from_issue_labels(labels: &[Label]) -> Self {
        let mut priorities: Vec<_> = labels
            .iter()
            .filter_map(|label| DeclaredPriority::parse(&label.name))
            .collect();
        priorities.sort_by_key(|priority| *priority as u8);
        priorities.dedup();
        match priorities.as_slice() {
            [] => Self::Unspecified,
            [priority] => Self::Declared { value: *priority },
            _ => Self::Conflict {
                labels: priorities
                    .into_iter()
                    .map(|priority| priority.canonical_label().to_owned())
                    .collect(),
            },
        }
    }

    pub(crate) fn comparison(&self) -> PriorityComparison {
        match self {
            Self::Declared {
                value: DeclaredPriority::P0,
            } => PriorityComparison::P0,
            Self::Declared {
                value: DeclaredPriority::P1,
            } => PriorityComparison::P1,
            Self::Declared {
                value: DeclaredPriority::P3,
            } => PriorityComparison::P3,
            Self::Declared {
                value: DeclaredPriority::P4,
            } => PriorityComparison::P4,
            Self::Declared {
                value: DeclaredPriority::P2,
            }
            | Self::Unspecified
            | Self::Conflict { .. } => PriorityComparison::Neutral,
        }
    }

    pub(crate) fn conflict_labels(&self) -> Option<&[String]> {
        match self {
            Self::Conflict { labels } => Some(labels),
            Self::Declared { .. } | Self::Unspecified => None,
        }
    }

    pub(crate) fn display_name(&self) -> &'static str {
        match self {
            Self::Declared { value } => value
                .spec()
                .name
                .strip_prefix("priority:")
                .expect("canonical Priority labels use the priority: prefix"),
            Self::Unspecified => "unspecified",
            Self::Conflict { .. } => "conflict",
        }
    }

    pub(crate) fn matches(&self, desired: Option<DeclaredPriority>) -> bool {
        match (self, desired) {
            (Self::Declared { value }, Some(desired)) => *value == desired,
            (Self::Unspecified, None) => true,
            (Self::Declared { .. } | Self::Conflict { .. }, None)
            | (Self::Unspecified | Self::Conflict { .. }, Some(_)) => false,
        }
    }
}

impl Serialize for PriorityState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (state, value, labels) = match self {
            Self::Declared { value } => ("declared", Some(*value), None),
            Self::Unspecified => ("unspecified", None, None),
            Self::Conflict { labels } => ("conflict", None, Some(labels.as_slice())),
        };
        PriorityStateOutput {
            state,
            comparison: self.comparison(),
            value,
            labels,
        }
        .serialize(serializer)
    }
}

#[derive(Serialize)]
struct PriorityStateOutput<'a> {
    state: &'static str,
    comparison: PriorityComparison,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<DeclaredPriority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<&'a [String]>,
}

pub(crate) fn present_canonical_labels(labels: &[Label]) -> Vec<&'static str> {
    DeclaredPriority::ALL
        .into_iter()
        .filter(|priority| {
            labels
                .iter()
                .any(|label| label.name.eq_ignore_ascii_case(priority.spec().name))
        })
        .map(DeclaredPriority::canonical_label)
        .collect()
}

pub(crate) fn missing_canonical_labels(labels: &[Label]) -> Vec<&'static str> {
    DeclaredPriority::ALL
        .into_iter()
        .filter(|priority| {
            !labels
                .iter()
                .any(|label| label.name.eq_ignore_ascii_case(priority.spec().name))
        })
        .map(DeclaredPriority::canonical_label)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{DeclaredPriority, PriorityComparison, PriorityState};

    #[test]
    fn every_declared_priority_has_a_stable_comparison_class() {
        let cases = [
            (DeclaredPriority::P0, PriorityComparison::P0),
            (DeclaredPriority::P1, PriorityComparison::P1),
            (DeclaredPriority::P2, PriorityComparison::Neutral),
            (DeclaredPriority::P3, PriorityComparison::P3),
            (DeclaredPriority::P4, PriorityComparison::P4),
        ];

        for (value, expected) in cases {
            assert_eq!(PriorityState::Declared { value }.comparison(), expected);
        }
    }
}
