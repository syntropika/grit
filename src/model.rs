use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub(crate) const REPLICA_SCHEMA_VERSION: &str = "grit.local-replica/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LocalReplica {
    pub(crate) schema_version: String,
    pub(crate) repository: String,
    pub(crate) synced_at: String,
    pub(crate) input_hash: String,
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
        issues: Vec<Issue>,
        dependencies: Vec<Dependency>,
    ) -> Result<Self, ReplicaError> {
        let input_hash = calculate_input_hash(&repository, &issues, &dependencies)?;
        Ok(Self {
            schema_version: REPLICA_SCHEMA_VERSION.to_owned(),
            repository,
            synced_at,
            input_hash,
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
        if let Some(cursor) = &self.sync.ordinary_issues {
            chrono::DateTime::parse_from_rfc3339(&cursor.watermark)
                .map_err(|_| ReplicaError::InvalidOrdinaryIssueWatermark)?;
        }
        let expected_hash =
            calculate_input_hash(&self.repository, &self.issues, &self.dependencies)?;
        if self.input_hash != expected_hash {
            return Err(ReplicaError::HashMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SyncMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ordinary_issues: Option<OrdinaryIssueCursor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OrdinaryIssueCursor {
    pub(crate) watermark: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) issues_etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) comments_etag: Option<String>,
}

fn calculate_input_hash(
    repository: &str,
    issues: &[Issue],
    dependencies: &[Dependency],
) -> Result<String, ReplicaError> {
    let input = HashInput {
        schema_version: REPLICA_SCHEMA_VERSION,
        repository,
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
    #[error("Local replica has an invalid ordinary-Issue watermark")]
    InvalidOrdinaryIssueWatermark,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockerScope {
    Internal,
    External,
}
