use chrono::{DateTime, Duration, FixedOffset, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::repository::IssueReference;

pub(crate) const REPLICA_SCHEMA_VERSION: &str = "grit.local-replica/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LocalReplica {
    pub(crate) schema_version: String,
    pub(crate) repository: String,
    pub(crate) synced_at: String,
    pub(crate) input_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repository_labels: Option<Vec<Label>>,
    #[serde(default)]
    pub(crate) sync: SyncMetadata,
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
}

impl LocalReplica {
    pub(crate) fn build_with_sync(
        repository: String,
        synced_at: String,
        sync: SyncMetadata,
        repository_labels: Vec<Label>,
        issues: Vec<Issue>,
        dependencies: Vec<Dependency>,
    ) -> Result<Self, ReplicaError> {
        let input_hash = calculate_input_hash(
            &repository,
            Some(&repository_labels),
            &issues,
            &dependencies,
        )?;
        Ok(Self {
            schema_version: REPLICA_SCHEMA_VERSION.to_owned(),
            repository,
            synced_at,
            input_hash,
            repository_labels: Some(repository_labels),
            sync,
            issues,
            dependencies,
        })
    }

    pub(crate) fn validate(&self, expected_repository: &str) -> Result<(), ReplicaError> {
        if self.schema_version != REPLICA_SCHEMA_VERSION {
            return Err(ReplicaError::UnsupportedSchema(self.schema_version.clone()));
        }
        if !self.repository.eq_ignore_ascii_case(expected_repository) {
            return Err(ReplicaError::RepositoryMismatch {
                expected: expected_repository.to_owned(),
                actual: self.repository.clone(),
            });
        }
        chrono::DateTime::parse_from_rfc3339(&self.synced_at)
            .map_err(|_| ReplicaError::InvalidSyncedAt)?;
        let expected_hash = calculate_input_hash(
            &self.repository,
            self.repository_labels.as_deref(),
            &self.issues,
            &self.dependencies,
        )?;
        let legacy_hash = self
            .repository_labels
            .is_none()
            .then(|| {
                calculate_legacy_input_hash(&self.repository, &self.issues, &self.dependencies)
            })
            .transpose()?;
        if self.input_hash != expected_hash && legacy_hash.as_ref() != Some(&self.input_hash) {
            return Err(ReplicaError::HashMismatch);
        }
        Ok(())
    }

    pub(crate) fn has_dependency(
        &self,
        blocked: &IssueReference,
        blocker: &IssueReference,
    ) -> bool {
        self.dependencies.iter().any(|dependency| {
            dependency
                .blocked
                .repository
                .eq_ignore_ascii_case(blocked.repository().full_name())
                && dependency.blocked.number == blocked.number()
                && dependency
                    .blocker
                    .repository
                    .eq_ignore_ascii_case(blocker.repository().full_name())
                && dependency.blocker.number == blocker.number()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SyncMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ordinary_issues: Option<OrdinaryIssueCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dependency_events: Option<DependencyEventCheckpoint>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DependencyEventCheckpoint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) latest_event_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OrdinaryIssueCursor {
    pub(crate) watermark: Watermark,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) issues_etag: Option<EntityTag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) comments_etag: Option<EntityTag>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Watermark(DateTime<FixedOffset>);

impl Watermark {
    pub(crate) fn now() -> Self {
        Self(Utc::now().fixed_offset())
    }

    pub(crate) fn overlapped_since(&self) -> String {
        (self.0 - Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    pub(crate) fn later(&self, other: &Self) -> Self {
        std::cmp::max(self, other).clone()
    }
}

impl Serialize for Watermark {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

impl<'de> Deserialize<'de> for Watermark {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&value)
            .map(Self)
            .map_err(|_| de::Error::custom("invalid ordinary-Issue watermark"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EntityTag(String);

impl EntityTag {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let opaque = value.strip_prefix("W/").unwrap_or(value);
        (value.is_ascii()
            && opaque.len() >= 2
            && opaque.starts_with('"')
            && opaque.ends_with('"')
            && !opaque[1..opaque.len() - 1]
                .chars()
                .any(|character| matches!(character, '\r' | '\n' | '"')))
        .then(|| Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for EntityTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EntityTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom("invalid GitHub ETag"))
    }
}

fn calculate_input_hash(
    repository: &str,
    repository_labels: Option<&[Label]>,
    issues: &[Issue],
    dependencies: &[Dependency],
) -> Result<String, ReplicaError> {
    let input = HashInput {
        schema_version: REPLICA_SCHEMA_VERSION,
        repository,
        repository_labels,
        issues,
        dependencies,
    };
    let canonical = serde_json::to_vec(&input).map_err(ReplicaError::EncodeHashInput)?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

#[derive(Serialize)]
struct HashInput<'a> {
    schema_version: &'static str,
    repository: &'a str,
    repository_labels: Option<&'a [Label]>,
    issues: &'a [Issue],
    dependencies: &'a [Dependency],
}

fn calculate_legacy_input_hash(
    repository: &str,
    issues: &[Issue],
    dependencies: &[Dependency],
) -> Result<String, ReplicaError> {
    let input = LegacyHashInput {
        schema_version: REPLICA_SCHEMA_VERSION,
        repository,
        issues,
        dependencies,
    };
    let canonical = serde_json::to_vec(&input).map_err(ReplicaError::EncodeHashInput)?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

#[derive(Serialize)]
struct LegacyHashInput<'a> {
    schema_version: &'static str,
    repository: &'a str,
    issues: &'a [Issue],
    dependencies: &'a [Dependency],
}

#[derive(Debug, Error)]
pub(crate) enum ReplicaError {
    #[error("could not encode normalized input for hashing: {0}")]
    EncodeHashInput(serde_json::Error),
    #[error("unsupported Local replica schema {0:?}")]
    UnsupportedSchema(String),
    #[error("Local replica belongs to {actual}, not {expected}")]
    RepositoryMismatch { expected: String, actual: String },
    #[error("Local replica has an invalid synced_at timestamp")]
    InvalidSyncedAt,
    #[error("Local replica input_hash does not match its normalized contents")]
    HashMismatch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Issue {
    pub(crate) id: u64,
    pub(crate) node_id: String,
    pub(crate) number: u64,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) state: String,
    pub(crate) state_reason: Option<String>,
    pub(crate) author: Option<Actor>,
    pub(crate) assignees: Vec<Actor>,
    pub(crate) labels: Vec<Label>,
    pub(crate) comments: Vec<Comment>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) closed_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Actor {
    pub(crate) id: u64,
    pub(crate) node_id: String,
    pub(crate) login: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Label {
    pub(crate) id: Option<u64>,
    pub(crate) node_id: Option<String>,
    pub(crate) name: String,
    pub(crate) color: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Comment {
    pub(crate) id: u64,
    pub(crate) node_id: String,
    pub(crate) url: String,
    pub(crate) body: String,
    pub(crate) author: Option<Actor>,
    pub(crate) author_association: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Dependency {
    pub(crate) blocked: IssueIdentity,
    pub(crate) blocker: BlockerIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct IssueIdentity {
    pub(crate) repository: String,
    pub(crate) number: u64,
    pub(crate) id: u64,
    pub(crate) node_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BlockerIdentity {
    pub(crate) repository: String,
    pub(crate) number: u64,
    pub(crate) state: String,
    pub(crate) scope: BlockerScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) node_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockerScope {
    Internal,
    External,
}
