use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub(crate) const REPLICA_SCHEMA_VERSION: &str = "grit.local-replica/v1";

pub(crate) fn strip_operation_markers(value: &str) -> String {
    let mut sanitized = value.to_owned();
    while let Some(start) = sanitized.find("<!-- grit:operation") {
        let Some(relative_end) = sanitized[start..].find("-->") else {
            sanitized.truncate(start);
            break;
        };
        sanitized.replace_range(start..start + relative_end + 3, "");
    }
    sanitized
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LocalReplica {
    pub(crate) schema_version: String,
    pub(crate) repository: String,
    pub(crate) synced_at: String,
    pub(crate) input_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repository_labels: Option<Vec<Label>>,
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
}

impl LocalReplica {
    pub(crate) fn build(
        repository: String,
        synced_at: String,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Dependency {
    pub(crate) blocked: IssueIdentity,
    pub(crate) blocker: BlockerIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct IssueIdentity {
    pub(crate) repository: String,
    pub(crate) number: u64,
    pub(crate) id: u64,
    pub(crate) node_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockerScope {
    Internal,
    External,
}
