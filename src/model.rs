use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::repository::IssueReference;

pub(crate) const REPLICA_SCHEMA_VERSION: &str = "grit.local-replica/v1";

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

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(from = "DependencyEdgeKeyWire")]
pub(crate) struct DependencyEdgeKey {
    blocked_repository: String,
    blocked_number: u64,
    blocker_repository: String,
    blocker_number: u64,
}

impl DependencyEdgeKey {
    pub(crate) fn new(
        blocked_repository: impl Into<String>,
        blocked_number: u64,
        blocker_repository: impl Into<String>,
        blocker_number: u64,
    ) -> Self {
        Self {
            blocked_repository: blocked_repository.into().to_ascii_lowercase(),
            blocked_number,
            blocker_repository: blocker_repository.into().to_ascii_lowercase(),
            blocker_number,
        }
    }

    pub(crate) fn from_dependency(dependency: &Dependency) -> Self {
        Self::new(
            &dependency.blocked.repository,
            dependency.blocked.number,
            &dependency.blocker.repository,
            dependency.blocker.number,
        )
    }

    pub(crate) fn blocked_repository(&self) -> &str {
        &self.blocked_repository
    }

    pub(crate) fn blocked_number(&self) -> u64 {
        self.blocked_number
    }

    pub(crate) fn blocker_repository(&self) -> &str {
        &self.blocker_repository
    }

    pub(crate) fn blocker_number(&self) -> u64 {
        self.blocker_number
    }

    pub(crate) fn is_internal(&self) -> bool {
        self.blocked_repository
            .eq_ignore_ascii_case(&self.blocker_repository)
    }
}

#[derive(Deserialize)]
struct DependencyEdgeKeyWire {
    blocked_repository: String,
    blocked_number: u64,
    blocker_repository: String,
    blocker_number: u64,
}

impl From<DependencyEdgeKeyWire> for DependencyEdgeKey {
    fn from(wire: DependencyEdgeKeyWire) -> Self {
        Self::new(
            wire.blocked_repository,
            wire.blocked_number,
            wire.blocker_repository,
            wire.blocker_number,
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DependencyPresence {
    Present,
    Absent,
}

impl DependencyPresence {
    pub(crate) fn from_present(present: bool) -> Self {
        if present { Self::Present } else { Self::Absent }
    }

    pub(crate) fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
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

#[cfg(test)]
mod tests {
    use super::DependencyEdgeKey;

    #[test]
    fn dependency_edge_keys_canonicalize_construction_and_deserialization() {
        let constructed = DependencyEdgeKey::new("Owner/Repo", 2, "OWNER/Other", 7);
        let decoded: DependencyEdgeKey = serde_json::from_value(serde_json::json!({
            "blocked_repository": "OWNER/REPO",
            "blocked_number": 2,
            "blocker_repository": "owner/OTHER",
            "blocker_number": 7
        }))
        .expect("edge key decodes");

        assert_eq!(constructed, decoded);
        assert_eq!(decoded.blocked_repository(), "owner/repo");
        assert_eq!(decoded.blocker_repository(), "owner/other");
    }
}
