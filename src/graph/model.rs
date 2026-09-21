use crate::model::{Issue, StableNodeKey, TemporaryIssueId};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct NodeKey {
    repository: String,
    identity: StableNodeKey,
}

impl NodeKey {
    pub(super) fn new(repository: &str, number: u64) -> Self {
        Self {
            repository: repository.to_owned(),
            identity: StableNodeKey::GitHub(number),
        }
    }

    pub(super) fn for_issue(repository: &str, issue: &Issue) -> Self {
        Self {
            repository: repository.to_owned(),
            identity: issue.stable_node_key(),
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        let (repository, identity) = value
            .rsplit_once('#')
            .ok_or_else(|| format!("invalid Stable node key {value}"))?;
        if repository.is_empty() {
            return Err(format!("invalid Stable node key {value}"));
        }
        let identity = if let Some(temporary_id) = identity.strip_prefix("draft:") {
            StableNodeKey::Draft(
                temporary_id
                    .parse()
                    .map_err(|_| format!("invalid Stable node key {value}"))?,
            )
        } else {
            let number = identity
                .parse::<u64>()
                .map_err(|_| format!("invalid Stable node key {value}"))?;
            if number == 0 {
                return Err(format!("invalid Stable node key {value}"));
            }
            StableNodeKey::GitHub(number)
        };
        Ok(Self {
            repository: repository.to_owned(),
            identity,
        })
    }

    pub(super) fn repository(&self) -> &str {
        &self.repository
    }

    pub(super) fn number(&self) -> Option<u64> {
        match self.identity {
            StableNodeKey::GitHub(number) => Some(number),
            StableNodeKey::Draft(_) => None,
        }
    }

    pub(super) fn temporary_id(&self) -> Option<TemporaryIssueId> {
        match self.identity {
            StableNodeKey::GitHub(_) => None,
            StableNodeKey::Draft(id) => Some(id),
        }
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.identity {
            StableNodeKey::GitHub(number) => write!(formatter, "{}#{number}", self.repository),
            StableNodeKey::Draft(id) => write!(formatter, "{}#draft:{id}", self.repository),
        }
    }
}

impl Serialize for NodeKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for NodeKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Position {
    pub(super) layer: Option<u32>,
    pub(super) x: i64,
    pub(super) y: i64,
}

pub(super) fn unresolved_position() -> Position {
    Position {
        layer: None,
        x: 0,
        y: 0,
    }
}
