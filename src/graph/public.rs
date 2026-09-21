mod render;
mod repository;
mod seal;
mod validation;

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    GraphError, SiteSummary,
    layout::{LayoutDependency, LayoutNode, dependency_positions},
    model::{NodeKey, Position, unresolved_position},
    publication,
    serialization::pretty_json,
    text::{sort_and_deduplicate, strip_operation_markers},
};
use crate::model::{BlockerScope, LocalReplica};

pub(crate) use repository::{ConfirmedPublicRepository, confirm_public_repository};

pub(crate) const PUBLIC_ARTIFACT_SCHEMA_VERSION: &str = "hyfa.public-graph/v1";

pub(crate) struct PublicGraphOptions {
    pub(crate) label_prefixes: Vec<String>,
    pub(crate) include_assignees: bool,
}

struct NormalizedOptions {
    label_prefixes: Vec<String>,
    include_assignees: bool,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum PublicSchemaVersion {
    #[serde(rename = "hyfa.public-graph/v1")]
    V1,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum SchemaLocation {
    #[serde(rename = "./graph.schema.json")]
    Local,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum PublicVisibility {
    #[serde(rename = "public")]
    Public,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicGraph {
    schema_version: PublicSchemaVersion,
    schema_url: SchemaLocation,
    repository: String,
    visibility: PublicVisibility,
    synced_at: String,
    public_input_hash: String,
    artifact_hash: String,
    operational_counts: PublicCounts,
    nodes: Vec<PublicNode>,
    edges: Vec<PublicEdge>,
}

#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicCounts {
    issue_count: usize,
    ready_count: usize,
    blocked_count: usize,
}

#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "public_node")]
struct PublicNode {
    #[schemars(with = "String")]
    key: NodeKey,
    number: u64,
    url: String,
    title: String,
    state: PublicIssueState,
    readiness: PublicReadiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignees: Option<Vec<String>>,
    position: Position,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum PublicIssueState {
    #[serde(rename = "open")]
    Open,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PublicReadiness {
    Ready,
    Blocked,
    ExternalUnknown,
}

#[derive(Clone, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "public_edge")]
struct PublicEdge {
    #[schemars(with = "String")]
    blocked: NodeKey,
    #[schemars(with = "String")]
    blocker: NodeKey,
    kind: PublicEdgeKind,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
enum PublicEdgeKind {
    #[serde(rename = "blocked_by")]
    BlockedBy,
}

pub(super) fn publish(
    replica: &LocalReplica,
    repository: &ConfirmedPublicRepository,
    options: &PublicGraphOptions,
    output: &std::path::Path,
) -> Result<SiteSummary, GraphError> {
    let options = normalize_options(options)?;
    let artifact = build(replica, repository, &options)?;
    let graph_bytes = pretty_json(&artifact)?;
    validation::validate_serialized(&graph_bytes, repository)?;
    let schema_bytes = render::schema_json(repository)?;
    let html_bytes = render::html(&artifact).into_bytes();
    let prohibited_values = seal::prohibited_values(replica, &artifact, repository)?;
    publication::publish_validated(
        output,
        &[
            ("graph.json", graph_bytes),
            ("graph.schema.json", schema_bytes),
            ("index.html", html_bytes),
        ],
        |staging| seal::validate(staging, repository, &prohibited_values),
    )?;
    Ok(SiteSummary {
        schema_version: PUBLIC_ARTIFACT_SCHEMA_VERSION,
        input_hash: artifact.public_input_hash.clone(),
        node_count: artifact.nodes.len(),
        edge_count: artifact.edges.len(),
        artifact_hash: artifact.artifact_hash,
    })
}

fn normalize_options(options: &PublicGraphOptions) -> Result<NormalizedOptions, GraphError> {
    if options
        .label_prefixes
        .iter()
        .any(|prefix| prefix.is_empty())
    {
        return Err(GraphError::EmptyPublicLabelPrefix);
    }
    let mut label_prefixes: Vec<_> = options
        .label_prefixes
        .iter()
        .map(|prefix| prefix.to_ascii_lowercase())
        .collect();
    label_prefixes.sort();
    label_prefixes.dedup();
    Ok(NormalizedOptions {
        label_prefixes,
        include_assignees: options.include_assignees,
    })
}

fn build(
    replica: &LocalReplica,
    repository: &ConfirmedPublicRepository,
    options: &NormalizedOptions,
) -> Result<PublicGraph, GraphError> {
    if !replica
        .repository
        .eq_ignore_ascii_case(&repository.full_name)
    {
        return Err(GraphError::PublicRepositoryMismatch {
            expected: repository.full_name.clone(),
            actual: replica.repository.clone(),
        });
    }

    let mut states = BTreeMap::new();
    let mut nodes = Vec::new();
    for issue in &replica.issues {
        let state = normalized_state(&issue.state)
            .ok_or(GraphError::InvalidPublicIssueState(issue.number))?;
        if states.insert(issue.number, state).is_some() {
            return Err(GraphError::DuplicateNode(
                NodeKey::new(&repository.full_name, issue.number).to_string(),
            ));
        }
        if state == "closed" {
            continue;
        }
        let labels = (!options.label_prefixes.is_empty()).then(|| {
            let mut labels: Vec<_> = issue
                .labels
                .iter()
                .map(|label| strip_operation_markers(&label.name))
                .filter(|label| {
                    let normalized = label.to_ascii_lowercase();
                    options
                        .label_prefixes
                        .iter()
                        .any(|prefix| normalized.starts_with(prefix))
                })
                .collect();
            sort_and_deduplicate(&mut labels);
            labels
        });
        let assignees = options.include_assignees.then(|| {
            let mut assignees: Vec<_> = issue
                .assignees
                .iter()
                .map(|actor| actor.login.clone())
                .collect();
            sort_and_deduplicate(&mut assignees);
            assignees
        });
        nodes.push(PublicNode {
            key: NodeKey::new(&repository.full_name, issue.number),
            number: issue.number,
            url: repository.issue_url(issue.number),
            title: strip_operation_markers(&issue.title),
            state: PublicIssueState::Open,
            readiness: PublicReadiness::Ready,
            labels,
            assignees,
            position: unresolved_position(),
        });
    }
    nodes.sort_by(|left, right| left.key.cmp(&right.key));
    let public_keys: BTreeSet<_> = nodes.iter().map(|node| node.key.clone()).collect();
    let mut external_unknown = BTreeSet::new();
    let mut edges = BTreeSet::new();
    for dependency in &replica.dependencies {
        if !dependency
            .blocked
            .repository
            .eq_ignore_ascii_case(&repository.full_name)
        {
            return Err(GraphError::DanglingInternalEndpoint(
                NodeKey::new(&dependency.blocked.repository, dependency.blocked.number).to_string(),
            ));
        }
        let Some(blocked_state) = states.get(&dependency.blocked.number) else {
            return Err(GraphError::DanglingInternalEndpoint(
                NodeKey::new(&repository.full_name, dependency.blocked.number).to_string(),
            ));
        };
        if *blocked_state == "closed" {
            continue;
        }
        match dependency.blocker.scope {
            BlockerScope::External => {
                if !dependency.blocker.state.eq_ignore_ascii_case("closed") {
                    external_unknown.insert(NodeKey::new(
                        &repository.full_name,
                        dependency.blocked.number,
                    ));
                }
            }
            BlockerScope::Internal => {
                if !dependency
                    .blocker
                    .repository
                    .eq_ignore_ascii_case(&repository.full_name)
                {
                    return Err(GraphError::DanglingInternalEndpoint(
                        NodeKey::new(&dependency.blocker.repository, dependency.blocker.number)
                            .to_string(),
                    ));
                }
                let Some(blocker_state) = states.get(&dependency.blocker.number) else {
                    return Err(GraphError::DanglingInternalEndpoint(
                        NodeKey::new(&repository.full_name, dependency.blocker.number).to_string(),
                    ));
                };
                if *blocker_state == "open" {
                    edges.insert(PublicEdge {
                        blocked: NodeKey::new(&repository.full_name, dependency.blocked.number),
                        blocker: NodeKey::new(&repository.full_name, dependency.blocker.number),
                        kind: PublicEdgeKind::BlockedBy,
                    });
                }
            }
        }
    }
    for edge in &edges {
        for endpoint in [&edge.blocked, &edge.blocker] {
            if !public_keys.contains(endpoint) {
                return Err(GraphError::DanglingEndpoint(endpoint.to_string()));
            }
        }
    }
    let edges: Vec<_> = edges.into_iter().collect();
    let blockers = blocker_index(&edges);
    for node in &mut nodes {
        node.readiness = if external_unknown.contains(&node.key) {
            PublicReadiness::ExternalUnknown
        } else if blockers
            .get(&node.key)
            .is_some_and(|values| !values.is_empty())
        {
            PublicReadiness::Blocked
        } else {
            PublicReadiness::Ready
        };
    }
    assign_public_positions(&mut nodes, &edges)?;
    let ready_count = nodes
        .iter()
        .filter(|node| node.readiness == PublicReadiness::Ready)
        .count();
    let operational_counts = PublicCounts {
        issue_count: nodes.len(),
        ready_count,
        blocked_count: nodes.len() - ready_count,
    };
    let public_input_hash = validation::calculate_public_input_hash(
        &repository.full_name,
        &operational_counts,
        &nodes,
        &edges,
    )?;
    let mut artifact = PublicGraph {
        schema_version: PublicSchemaVersion::V1,
        schema_url: SchemaLocation::Local,
        repository: repository.full_name.clone(),
        visibility: PublicVisibility::Public,
        synced_at: replica.synced_at.clone(),
        public_input_hash,
        artifact_hash: String::new(),
        operational_counts,
        nodes,
        edges,
    };
    artifact.artifact_hash = validation::calculate_artifact_hash(&artifact)?;
    validation::validate(&artifact, repository)?;
    Ok(artifact)
}

fn assign_public_positions(
    nodes: &mut [PublicNode],
    edges: &[PublicEdge],
) -> Result<(), GraphError> {
    let layout_nodes: Vec<_> = nodes
        .iter()
        .map(|node| {
            LayoutNode::eligible(
                node.key.clone(),
                node.readiness == PublicReadiness::ExternalUnknown,
            )
        })
        .collect();
    let dependencies: Vec<_> = edges
        .iter()
        .map(|edge| LayoutDependency::new(edge.blocked.clone(), edge.blocker.clone()))
        .collect();
    let positions = dependency_positions(&layout_nodes, &dependencies)?;
    for node in nodes {
        node.position = positions
            .get(&node.key)
            .cloned()
            .ok_or_else(|| GraphError::DanglingEndpoint(node.key.to_string()))?;
    }
    Ok(())
}

fn blocker_index(edges: &[PublicEdge]) -> BTreeMap<NodeKey, Vec<NodeKey>> {
    let mut blockers = BTreeMap::<NodeKey, Vec<NodeKey>>::new();
    for edge in edges {
        blockers
            .entry(edge.blocked.clone())
            .or_default()
            .push(edge.blocker.clone());
    }
    blockers
}

fn normalized_state(value: &str) -> Option<&'static str> {
    if value.eq_ignore_ascii_case("open") {
        Some("open")
    } else if value.eq_ignore_ascii_case("closed") {
        Some("closed")
    } else {
        None
    }
}
