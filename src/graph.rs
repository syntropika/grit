mod artifact;
mod layout;
mod publication;
mod render;

use std::{io, path::Path};

use thiserror::Error;

use crate::model::LocalReplica;

pub(crate) use artifact::ARTIFACT_SCHEMA_VERSION;

pub(crate) struct SiteSummary {
    pub(crate) node_count: usize,
    pub(crate) edge_count: usize,
    pub(crate) artifact_hash: String,
}

pub(crate) fn publish_site(
    replica: &LocalReplica,
    output: &Path,
) -> Result<SiteSummary, GraphError> {
    let artifact = artifact::build(replica)?;
    let graph_bytes = render::graph_json(&artifact)?;
    artifact::validate_serialized(&graph_bytes)?;
    let schema_bytes = render::schema_json()?;
    let html_bytes = render::html(&artifact)?.into_bytes();

    publication::publish(
        output,
        &[
            ("graph.json", graph_bytes),
            ("graph.schema.json", schema_bytes),
            ("index.html", html_bytes),
            ("app.css", render::stylesheet().to_vec()),
            ("graph-query.js", render::graph_query_javascript().to_vec()),
            ("app.js", render::javascript().to_vec()),
        ],
    )?;

    Ok(SiteSummary {
        node_count: artifact.nodes.len(),
        edge_count: artifact.edges.len(),
        artifact_hash: artifact.artifact_hash,
    })
}

#[derive(Debug, Error)]
pub(crate) enum GraphError {
    #[error("graph contains duplicate node {0}")]
    DuplicateNode(String),
    #[error("graph contains a dangling internal Dependency endpoint {0}")]
    DanglingInternalEndpoint(String),
    #[error("graph contains a dangling Dependency endpoint {0}")]
    DanglingEndpoint(String),
    #[error("graph contains inconsistent state for External blocker {0}")]
    InconsistentExternalState(String),
    #[error("graph contains invalid Stable node key {0}")]
    InvalidStableKey(String),
    #[error("graph artifact schema identity is invalid")]
    InvalidSchemaIdentity,
    #[error("graph artifact field {0} failed validation")]
    InvalidField(&'static str),
    #[error("graph artifact operational counts are inconsistent")]
    InvalidOperationalCounts,
    #[error("graph artifact provenance is inconsistent")]
    InvalidProvenance,
    #[error("graph artifact nodes are not unique and deterministically ordered")]
    NonDeterministicNodeOrder,
    #[error("graph artifact edges are not unique and deterministically ordered")]
    NonDeterministicEdgeOrder,
    #[error("graph artifact hash does not match its normalized contents")]
    ArtifactHashMismatch,
    #[error("could not encode the graph artifact: {0}")]
    EncodeArtifact(serde_json::Error),
    #[error("graph explorer HTML template contains invalid placeholder {0}")]
    InvalidHtmlTemplate(String),
    #[error("graph artifact failed closed-schema validation: {0}")]
    ValidateSchema(serde_json::Error),
    #[error("graph output must name a specific child directory")]
    UnsafeOutputPath,
    #[error("graph output must be a real directory, not a file or symbolic link")]
    UnsafeOutputTarget,
    #[error("could not create the graph output parent: {0}")]
    CreateParent(io::Error),
    #[error("could not create a graph staging directory: {0}")]
    CreateStaging(io::Error),
    #[error("could not find a unique graph staging directory name")]
    StagingNameExhausted,
    #[error("could not write a graph artifact: {0}")]
    WriteArtifact(io::Error),
    #[error("could not atomically publish the complete graph site: {0}")]
    Publish(io::Error),
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    #[error("atomic graph-site replacement is unavailable on this platform")]
    AtomicReplacementUnavailable,
    #[error("published graph site is valid, but the previous site could not be removed: {0}")]
    RemovePrevious(io::Error),
}
