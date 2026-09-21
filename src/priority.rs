use std::{borrow::Cow, fmt, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::model::Label;

pub(crate) struct PriorityLabelSpec {
    pub(crate) name: &'static str,
    pub(crate) color: &'static str,
    pub(crate) description: &'static str,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeclaredPriority {
    P0,
    P1,
    P2,
    P3,
    P4,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum LogicalPriority {
    Declared { value: DeclaredPriority },
    Unspecified,
    Conflict { labels: Vec<String> },
}

impl LogicalPriority {
    pub(crate) fn from_state(state: &PriorityState) -> Self {
        match state {
            PriorityState::Declared { value } => Self::Declared { value: *value },
            PriorityState::Unspecified => Self::Unspecified,
            PriorityState::Conflict { values } => Self::Conflict {
                labels: values
                    .iter()
                    .map(|value| value.canonical_label().to_owned())
                    .collect(),
            },
        }
    }

    pub(crate) fn from_selection(selection: PrioritySelection) -> Self {
        match selection {
            PrioritySelection::Declared(value) => Self::Declared { value },
            PrioritySelection::None => Self::Unspecified,
        }
    }

    pub(crate) fn to_state(&self) -> PriorityState {
        match self {
            Self::Declared { value } => PriorityState::Declared { value: *value },
            Self::Unspecified => PriorityState::Unspecified,
            Self::Conflict { labels } => PriorityState::Conflict {
                values: labels
                    .iter()
                    .filter_map(|label| DeclaredPriority::parse(label))
                    .collect(),
            },
        }
    }

    pub(crate) fn from_canonical_labels(labels: &[String]) -> Option<Self> {
        let priorities: Option<Vec<_>> = labels
            .iter()
            .map(|label| {
                DeclaredPriority::parse(label)
                    .filter(|priority| priority.canonical_label() == label)
            })
            .collect();
        let priorities = priorities?;
        if labels.windows(2).any(|pair| pair[0] >= pair[1]) {
            return None;
        }
        Some(match priorities.as_slice() {
            [] => Self::Unspecified,
            [value] => Self::Declared { value: *value },
            _ => Self::Conflict {
                labels: labels.to_vec(),
            },
        })
    }
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

    fn comparison(self) -> PriorityComparison {
        match self {
            Self::P0 => PriorityComparison::P0,
            Self::P1 => PriorityComparison::P1,
            Self::P2 => PriorityComparison::Neutral,
            Self::P3 => PriorityComparison::P3,
            Self::P4 => PriorityComparison::P4,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PriorityComparison {
    P0,
    P1,
    Neutral,
    P3,
    P4,
}

impl PriorityComparison {
    fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::Neutral => "neutral",
            Self::P3 => "p3",
            Self::P4 => "p4",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PriorityState {
    Declared { value: DeclaredPriority },
    Unspecified,
    Conflict { values: Vec<DeclaredPriority> },
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
            [value] => Self::Declared { value: *value },
            _ => Self::Conflict { values: priorities },
        }
    }

    pub(crate) fn comparison(&self) -> PriorityComparison {
        match self {
            Self::Declared { value } => value.comparison(),
            Self::Unspecified | Self::Conflict { .. } => PriorityComparison::Neutral,
        }
    }

    pub(crate) fn conflict_labels(&self) -> Option<Vec<String>> {
        match self {
            Self::Conflict { values } => Some(
                values
                    .iter()
                    .map(|value| value.canonical_label().to_owned())
                    .collect(),
            ),
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PriorityStatus {
    Declared,
    Unspecified,
    Conflict,
}

impl Serialize for PriorityState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            state: PriorityStatus,
            comparison: PriorityComparison,
            #[serde(skip_serializing_if = "Option::is_none")]
            value: Option<DeclaredPriority>,
            #[serde(skip_serializing_if = "Option::is_none")]
            labels: Option<Vec<&'a str>>,
        }

        let (state, value, labels) = match self {
            Self::Declared { value } => (PriorityStatus::Declared, Some(*value), None),
            Self::Unspecified => (PriorityStatus::Unspecified, None, None),
            Self::Conflict { values } => (
                PriorityStatus::Conflict,
                None,
                Some(values.iter().map(|value| value.canonical_label()).collect()),
            ),
        };
        Wire {
            state,
            comparison: self.comparison(),
            value,
            labels,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PriorityState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            state: PriorityStatus,
            comparison: PriorityComparison,
            value: Option<DeclaredPriority>,
            labels: Option<Vec<String>>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let state = match (wire.state, wire.value, wire.labels) {
            (PriorityStatus::Declared, Some(value), None) => Self::Declared { value },
            (PriorityStatus::Unspecified, None, None) => Self::Unspecified,
            (PriorityStatus::Conflict, None, Some(labels)) => {
                let mut values: Vec<_> = labels
                    .iter()
                    .map(|label| {
                        DeclaredPriority::parse(label)
                            .filter(|value| value.canonical_label() == label)
                    })
                    .collect::<Option<_>>()
                    .ok_or_else(|| de::Error::custom("invalid Priority state"))?;
                values.sort_by_key(|value| *value as u8);
                values.dedup();
                if values.len() != labels.len() || values.len() < 2 {
                    return Err(de::Error::custom("invalid Priority state"));
                }
                Self::Conflict { values }
            }
            _ => return Err(de::Error::custom("invalid Priority state")),
        };
        if wire.comparison != state.comparison() {
            return Err(de::Error::custom("invalid Priority state"));
        }
        Ok(state)
    }
}

impl JsonSchema for PriorityState {
    fn schema_name() -> Cow<'static, str> {
        "PriorityState".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::PriorityState").into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        let mut variants: Vec<_> = DeclaredPriority::ALL
            .into_iter()
            .map(|value| {
                let value_name = value.canonical_label().trim_start_matches("priority:");
                let comparison = value.comparison().as_str();
                json_schema!({
                "type": "object",
                "additionalProperties": false,
                "required": ["state", "comparison", "value"],
                "properties": {
                    "state": { "const": "declared" },
                        "comparison": { "const": comparison },
                        "value": { "const": value_name }
                    }
                })
            })
            .collect();
        let labels: Vec<_> = DeclaredPriority::ALL
            .into_iter()
            .map(DeclaredPriority::canonical_label)
            .collect();
        variants.push(json_schema!({
            "type": "object",
            "additionalProperties": false,
            "required": ["state", "comparison"],
            "properties": {
                "state": { "const": "unspecified" },
                "comparison": { "const": "neutral" }
            }
        }));
        variants.push(json_schema!({
            "type": "object",
            "additionalProperties": false,
            "required": ["state", "comparison", "labels"],
            "properties": {
                "state": { "const": "conflict" },
                "comparison": { "const": "neutral" },
                "labels": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 5,
                    "uniqueItems": true,
                    "items": { "enum": labels }
                }
            }
        }));
        json_schema!({ "oneOf": variants })
    }
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
    use schemars::schema_for;
    use serde_json::{Value, json};

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
            let labels = [crate::model::Label {
                id: Some(1),
                node_id: Some("L_1".to_owned()),
                name: value.spec().name.to_owned(),
                color: Some("000000".to_owned()),
                description: None,
            }];
            assert_eq!(
                PriorityState::from_issue_labels(&labels).comparison(),
                expected
            );
        }
    }

    #[test]
    fn priority_schema_and_deserializer_reject_invalid_state_combinations() {
        let schema = serde_json::to_value(schema_for!(PriorityState)).expect("Priority schema");
        assert_eq!(
            schema["oneOf"].as_array().expect("Priority variants").len(),
            7
        );
        for variant in schema["oneOf"].as_array().expect("Priority variants") {
            assert_eq!(variant["additionalProperties"], false);
        }

        let invalid = json!({"state": "declared", "comparison": "neutral", "value": "p0"});
        assert!(serde_json::from_value::<PriorityState>(invalid).is_err());
        let valid: PriorityState = serde_json::from_value(json!({
            "state": "conflict",
            "comparison": "neutral",
            "labels": ["priority:p4", "priority:p0"]
        }))
        .expect("schema-valid conflict");
        let normalized: Value = serde_json::to_value(valid).expect("normalized Priority");
        assert_eq!(normalized["labels"], json!(["priority:p0", "priority:p4"]));
    }
}
