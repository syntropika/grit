use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct NodeKey {
    repository: String,
    number: u64,
}

impl NodeKey {
    pub(super) fn new(repository: &str, number: u64) -> Self {
        Self {
            repository: repository.to_owned(),
            number,
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        let (repository, number) = value
            .rsplit_once('#')
            .ok_or_else(|| format!("invalid Stable node key {value}"))?;
        let number: u64 = number
            .parse()
            .map_err(|_| format!("invalid Stable node key {value}"))?;
        if repository.is_empty() || number == 0 {
            return Err(format!("invalid Stable node key {value}"));
        }
        Ok(Self::new(repository, number))
    }

    pub(super) fn repository(&self) -> &str {
        &self.repository
    }

    pub(super) fn number(&self) -> u64 {
        self.number
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}#{}", self.repository, self.number)
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
