use std::collections::BTreeSet;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    ConfirmedPublicRepository, PublicCounts, PublicEdge, PublicGraph, PublicNode, PublicReadiness,
    PublicSchemaVersion, PublicVisibility, SchemaLocation, assign_public_positions, blocker_index,
};
use crate::graph::{GraphError, model::NodeKey};

pub(super) fn validate_serialized(
    bytes: &[u8],
    repository: &ConfirmedPublicRepository,
) -> Result<(), GraphError> {
    decode_and_validate(bytes, repository).map(|_| ())
}

pub(super) fn decode_and_validate(
    bytes: &[u8],
    repository: &ConfirmedPublicRepository,
) -> Result<PublicGraph, GraphError> {
    let artifact: PublicGraph =
        serde_json::from_slice(bytes).map_err(GraphError::ValidateSchema)?;
    validate(&artifact, repository)?;
    Ok(artifact)
}

pub(super) fn validate(
    artifact: &PublicGraph,
    repository: &ConfirmedPublicRepository,
) -> Result<(), GraphError> {
    if artifact.schema_version != PublicSchemaVersion::V1
        || artifact.schema_url != SchemaLocation::Local
        || artifact.visibility != PublicVisibility::Public
    {
        return Err(GraphError::InvalidSchemaIdentity);
    }
    if artifact.repository != repository.full_name {
        return Err(GraphError::PublicRepositoryMismatch {
            expected: repository.full_name.clone(),
            actual: artifact.repository.clone(),
        });
    }
    chrono::DateTime::parse_from_rfc3339(&artifact.synced_at)
        .map_err(|_| GraphError::InvalidField("synced_at"))?;
    validate_hash("public_input_hash", &artifact.public_input_hash)?;
    validate_hash("artifact_hash", &artifact.artifact_hash)?;
    if !artifact
        .nodes
        .windows(2)
        .all(|pair| pair[0].key < pair[1].key)
    {
        return Err(GraphError::NonDeterministicNodeOrder);
    }
    let keys: BTreeSet<_> = artifact.nodes.iter().map(|node| &node.key).collect();
    let blockers = blocker_index(&artifact.edges);
    for node in &artifact.nodes {
        if node.key != NodeKey::new(&repository.full_name, node.number) {
            return Err(GraphError::InvalidStableKey(node.key.to_string()));
        }
        if node.url != repository.issue_url(node.number) {
            return Err(GraphError::InvalidCanonicalIssueUrl(node.number));
        }
        if [
            "<!-- hyfa:operation",
            "<!-- hyfa-operation:",
            "<!-- grit:operation",
            "<!-- grit-operation:",
        ]
        .iter()
        .any(|prefix| node.title.contains(prefix))
        {
            return Err(GraphError::InvalidField("title"));
        }
        if let Some(labels) = &node.labels {
            validate_ordered_text(labels, "labels")?;
        }
        if let Some(assignees) = &node.assignees {
            validate_ordered_text(assignees, "assignees")?;
        }
        let has_blockers = blockers
            .get(&node.key)
            .is_some_and(|values| !values.is_empty());
        match node.readiness {
            PublicReadiness::Ready if has_blockers => {
                return Err(GraphError::InvalidField("readiness"));
            }
            PublicReadiness::Blocked if !has_blockers => {
                return Err(GraphError::InvalidField("readiness"));
            }
            PublicReadiness::Ready
            | PublicReadiness::Blocked
            | PublicReadiness::ExternalUnknown => {}
        }
    }
    if !artifact.edges.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(GraphError::NonDeterministicEdgeOrder);
    }
    for edge in &artifact.edges {
        if !keys.contains(&edge.blocked) {
            return Err(GraphError::DanglingEndpoint(edge.blocked.to_string()));
        }
        if !keys.contains(&edge.blocker) {
            return Err(GraphError::DanglingEndpoint(edge.blocker.to_string()));
        }
    }
    let mut positioned = artifact.nodes.clone();
    assign_public_positions(&mut positioned, &artifact.edges)?;
    if positioned != artifact.nodes {
        return Err(GraphError::InvalidField("position"));
    }
    let ready_count = artifact
        .nodes
        .iter()
        .filter(|node| node.readiness == PublicReadiness::Ready)
        .count();
    if artifact.operational_counts.issue_count != artifact.nodes.len()
        || artifact.operational_counts.ready_count != ready_count
        || artifact.operational_counts.blocked_count != artifact.nodes.len() - ready_count
    {
        return Err(GraphError::InvalidOperationalCounts);
    }
    let input_hash = calculate_public_input_hash(
        &artifact.repository,
        &artifact.operational_counts,
        &artifact.nodes,
        &artifact.edges,
    )?;
    if input_hash != artifact.public_input_hash {
        return Err(GraphError::ArtifactHashMismatch);
    }
    if calculate_artifact_hash(artifact)? != artifact.artifact_hash {
        return Err(GraphError::ArtifactHashMismatch);
    }
    Ok(())
}

#[derive(Serialize)]
struct PublicInputHash<'a> {
    schema_version: PublicSchemaVersion,
    repository: &'a str,
    operational_counts: &'a PublicCounts,
    nodes: &'a [PublicNode],
    edges: &'a [PublicEdge],
}

pub(super) fn calculate_public_input_hash(
    repository: &str,
    operational_counts: &PublicCounts,
    nodes: &[PublicNode],
    edges: &[PublicEdge],
) -> Result<String, GraphError> {
    hash(&PublicInputHash {
        schema_version: PublicSchemaVersion::V1,
        repository,
        operational_counts,
        nodes,
        edges,
    })
}

#[derive(Serialize)]
struct PublicArtifactHash<'a> {
    schema_version: PublicSchemaVersion,
    repository: &'a str,
    visibility: PublicVisibility,
    public_input_hash: &'a str,
    operational_counts: &'a PublicCounts,
    nodes: &'a [PublicNode],
    edges: &'a [PublicEdge],
}

pub(super) fn calculate_artifact_hash(artifact: &PublicGraph) -> Result<String, GraphError> {
    hash(&PublicArtifactHash {
        schema_version: artifact.schema_version,
        repository: &artifact.repository,
        visibility: artifact.visibility,
        public_input_hash: &artifact.public_input_hash,
        operational_counts: &artifact.operational_counts,
        nodes: &artifact.nodes,
        edges: &artifact.edges,
    })
}

fn hash<T: Serialize>(value: &T) -> Result<String, GraphError> {
    let bytes = serde_json::to_vec(value).map_err(GraphError::EncodeArtifact)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn validate_hash(name: &'static str, value: &str) -> Result<(), GraphError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(GraphError::InvalidField(name));
    }
    Ok(())
}

fn validate_ordered_text(values: &[String], field: &'static str) -> Result<(), GraphError> {
    if values
        .windows(2)
        .any(|pair| pair[0].to_ascii_lowercase() >= pair[1].to_ascii_lowercase())
    {
        return Err(GraphError::InvalidField(field));
    }
    Ok(())
}
