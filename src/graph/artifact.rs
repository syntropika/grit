use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

use super::GraphError;
use crate::{
    model::{BlockerScope, LocalReplica},
    operational::{ExecutionScope, analyze_ready},
};

pub(crate) const ARTIFACT_SCHEMA_VERSION: &str = "grit.graph-artifact/v1";

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum ArtifactSchemaVersion {
    #[serde(rename = "grit.graph-artifact/v1")]
    V1,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum SchemaLocation {
    #[serde(rename = "./graph.schema.json")]
    Local,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraphArtifact {
    schema_version: ArtifactSchemaVersion,
    schema_url: SchemaLocation,
    pub(super) repository: String,
    pub(super) synced_at: String,
    input_hash: String,
    effective_input_hash: String,
    pub(super) artifact_hash: String,
    provenance: ArtifactProvenance,
    operational_counts: OperationalCounts,
    pub(super) nodes: Vec<ArtifactNode>,
    pub(super) edges: Vec<ArtifactEdge>,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct ArtifactProvenance {
    base: ProvenanceState,
    state: ProvenanceState,
    pending_mutation_count: u64,
    pending_operation_ids: Vec<String>,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProvenanceState {
    Synchronized,
    Pending,
}

#[derive(Clone, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct ElementProvenance {
    state: ProvenanceState,
    operation_ids: Vec<String>,
}

impl ElementProvenance {
    fn synchronized() -> Self {
        Self {
            state: ProvenanceState::Synchronized,
            operation_ids: Vec::new(),
        }
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct OperationalCounts {
    operational_issue_count: usize,
    ready_count: usize,
    executable_count: usize,
    assigned_ready_count: usize,
    blocked_count: usize,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "node")]
pub(super) struct ArtifactNode {
    #[schemars(with = "String")]
    pub(super) key: NodeKey,
    repository: String,
    number: u64,
    pub(super) kind: NodeKind,
    pub(super) url: Option<String>,
    pub(super) title: Option<String>,
    pub(super) state: String,
    pub(super) readiness: Readiness,
    pub(super) assignees: Vec<String>,
    pub(super) labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) projects: Option<Vec<String>>,
    pub(super) position: Position,
    provenance: ElementProvenance,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum NodeKind {
    Issue,
    ExternalBlocker,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Readiness {
    Ready,
    Blocked,
    Closed,
    Unknown,
    ExternalOpen,
    ExternalClosed,
    ExternalUnknown,
}

impl Readiness {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
            Self::ExternalOpen => "external_open",
            Self::ExternalClosed => "external_closed",
            Self::ExternalUnknown => "external_unknown",
        }
    }
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Position {
    pub(super) layer: Option<u32>,
    pub(super) x: i64,
    pub(super) y: i64,
}

#[derive(Clone, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "edge")]
pub(super) struct ArtifactEdge {
    #[schemars(with = "String")]
    pub(super) blocked: NodeKey,
    #[schemars(with = "String")]
    pub(super) blocker: NodeKey,
    kind: EdgeKind,
    provenance: ElementProvenance,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum EdgeKind {
    BlockedBy,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct NodeKey {
    repository: String,
    number: u64,
}

impl NodeKey {
    fn new(repository: &str, number: u64) -> Self {
        Self {
            repository: repository.to_owned(),
            number,
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
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

pub(super) fn build(replica: &LocalReplica) -> Result<GraphArtifact, GraphError> {
    let analysis = analyze_ready(replica, ExecutionScope::Available);
    let ready_numbers: BTreeSet<_> = analysis.ready.iter().map(|issue| issue.number).collect();
    let mut nodes = Vec::new();
    let mut node_keys = BTreeSet::new();

    for issue in &replica.issues {
        let key = NodeKey::new(&replica.repository, issue.number);
        if !node_keys.insert(key.clone()) {
            return Err(GraphError::DuplicateNode(key.to_string()));
        }
        let state = normalized_state(&issue.state);
        let readiness = if state == "closed" {
            Readiness::Closed
        } else if state != "open" {
            Readiness::Unknown
        } else if ready_numbers.contains(&issue.number) {
            Readiness::Ready
        } else {
            Readiness::Blocked
        };
        let mut assignees: Vec<_> = issue
            .assignees
            .iter()
            .map(|actor| actor.login.clone())
            .collect();
        sort_and_deduplicate(&mut assignees);
        let mut labels: Vec<_> = issue
            .labels
            .iter()
            .map(|label| strip_operation_markers(&label.name))
            .collect();
        sort_and_deduplicate(&mut labels);
        nodes.push(ArtifactNode {
            key,
            repository: replica.repository.clone(),
            number: issue.number,
            kind: NodeKind::Issue,
            url: Some(issue.url.clone()),
            title: Some(strip_operation_markers(&issue.title)),
            state: state.to_owned(),
            readiness,
            assignees,
            labels,
            projects: None,
            position: unresolved_position(),
            provenance: ElementProvenance::synchronized(),
        });
    }

    let mut external_states = BTreeMap::new();
    let mut edges = BTreeSet::new();
    for dependency in &replica.dependencies {
        if !dependency
            .blocked
            .repository
            .eq_ignore_ascii_case(&replica.repository)
        {
            return Err(GraphError::DanglingInternalEndpoint(
                NodeKey::new(&dependency.blocked.repository, dependency.blocked.number).to_string(),
            ));
        }
        let blocked = NodeKey::new(&replica.repository, dependency.blocked.number);
        if !node_keys.contains(&blocked) {
            return Err(GraphError::DanglingInternalEndpoint(blocked.to_string()));
        }
        let blocker = match dependency.blocker.scope {
            BlockerScope::Internal => {
                if !dependency
                    .blocker
                    .repository
                    .eq_ignore_ascii_case(&replica.repository)
                {
                    return Err(GraphError::DanglingInternalEndpoint(
                        NodeKey::new(&dependency.blocker.repository, dependency.blocker.number)
                            .to_string(),
                    ));
                }
                let blocker = NodeKey::new(&replica.repository, dependency.blocker.number);
                if !node_keys.contains(&blocker) {
                    return Err(GraphError::DanglingInternalEndpoint(blocker.to_string()));
                }
                blocker
            }
            BlockerScope::External => {
                let blocker =
                    NodeKey::new(&dependency.blocker.repository, dependency.blocker.number);
                let state = normalized_state(&dependency.blocker.state).to_owned();
                if let Some(existing) = external_states.insert(blocker.clone(), state.clone())
                    && existing != state
                {
                    return Err(GraphError::InconsistentExternalState(blocker.to_string()));
                }
                blocker
            }
        };
        edges.insert(ArtifactEdge {
            blocked,
            blocker,
            kind: EdgeKind::BlockedBy,
            provenance: ElementProvenance::synchronized(),
        });
    }

    for (key, state) in external_states {
        if node_keys.contains(&key) {
            return Err(GraphError::DuplicateNode(key.to_string()));
        }
        nodes.push(ArtifactNode {
            repository: key.repository.clone(),
            number: key.number,
            key: key.clone(),
            kind: NodeKind::ExternalBlocker,
            url: None,
            title: None,
            state: state.clone(),
            readiness: match state.as_str() {
                "open" => Readiness::ExternalOpen,
                "closed" => Readiness::ExternalClosed,
                _ => Readiness::ExternalUnknown,
            },
            assignees: Vec::new(),
            labels: Vec::new(),
            projects: None,
            position: unresolved_position(),
            provenance: ElementProvenance::synchronized(),
        });
        node_keys.insert(key);
    }

    nodes.sort_by(|left, right| left.key.cmp(&right.key));
    let edges: Vec<_> = edges.into_iter().collect();
    super::layout::assign_dependency_layers(&mut nodes, &edges)?;
    let provenance = ArtifactProvenance {
        base: ProvenanceState::Synchronized,
        state: ProvenanceState::Synchronized,
        pending_mutation_count: 0,
        pending_operation_ids: Vec::new(),
    };
    let operational_counts = OperationalCounts {
        operational_issue_count: analysis.operational_issue_count,
        ready_count: analysis.ready_count,
        executable_count: analysis.executable.len(),
        assigned_ready_count: analysis.assigned_ready_count,
        blocked_count: analysis.blocked_count,
    };
    let mut artifact = GraphArtifact {
        schema_version: ArtifactSchemaVersion::V1,
        schema_url: SchemaLocation::Local,
        repository: replica.repository.clone(),
        synced_at: replica.synced_at.clone(),
        input_hash: replica.input_hash.clone(),
        effective_input_hash: replica.input_hash.clone(),
        artifact_hash: String::new(),
        provenance,
        operational_counts,
        nodes,
        edges,
    };
    artifact.artifact_hash = calculate_hash(&artifact)?;
    validate(&artifact)?;
    Ok(artifact)
}

pub(super) fn validate_serialized(bytes: &[u8]) -> Result<(), GraphError> {
    let artifact: GraphArtifact =
        serde_json::from_slice(bytes).map_err(GraphError::ValidateSchema)?;
    validate(&artifact)
}

fn validate(artifact: &GraphArtifact) -> Result<(), GraphError> {
    if artifact.schema_version != ArtifactSchemaVersion::V1
        || artifact.schema_url != SchemaLocation::Local
    {
        return Err(GraphError::InvalidSchemaIdentity);
    }
    chrono::DateTime::parse_from_rfc3339(&artifact.synced_at)
        .map_err(|_| GraphError::InvalidField("synced_at"))?;
    for (name, value) in [
        ("input_hash", artifact.input_hash.as_str()),
        (
            "effective_input_hash",
            artifact.effective_input_hash.as_str(),
        ),
        ("artifact_hash", artifact.artifact_hash.as_str()),
    ] {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(GraphError::InvalidField(name));
        }
    }
    if artifact.operational_counts.ready_count + artifact.operational_counts.blocked_count
        != artifact.operational_counts.operational_issue_count
        || artifact.operational_counts.executable_count > artifact.operational_counts.ready_count
        || artifact.operational_counts.assigned_ready_count
            > artifact.operational_counts.ready_count
    {
        return Err(GraphError::InvalidOperationalCounts);
    }
    validate_provenance(
        artifact.provenance.state,
        &artifact.provenance.pending_operation_ids,
    )?;
    if artifact.provenance.base != ProvenanceState::Synchronized
        || artifact.provenance.pending_mutation_count as usize
            != artifact.provenance.pending_operation_ids.len()
    {
        return Err(GraphError::InvalidProvenance);
    }
    let pending_ids: BTreeSet<_> = artifact
        .provenance
        .pending_operation_ids
        .iter()
        .map(String::as_str)
        .collect();

    if !artifact
        .nodes
        .windows(2)
        .all(|pair| pair[0].key < pair[1].key)
    {
        return Err(GraphError::NonDeterministicNodeOrder);
    }
    let key_set: BTreeSet<_> = artifact.nodes.iter().map(|node| &node.key).collect();
    for node in &artifact.nodes {
        if node.number != node.key.number || node.repository != node.key.repository {
            return Err(GraphError::InvalidStableKey(node.key.to_string()));
        }
        match node.kind {
            NodeKind::Issue
                if !node.repository.eq_ignore_ascii_case(&artifact.repository)
                    || node.url.is_none()
                    || node.title.is_none() =>
            {
                return Err(GraphError::InvalidField("nodes"));
            }
            NodeKind::ExternalBlocker if node.url.is_some() || node.title.is_some() => {
                return Err(GraphError::InvalidField("nodes"));
            }
            NodeKind::Issue | NodeKind::ExternalBlocker => {}
        }
        if !valid_projects(node) {
            return Err(GraphError::InvalidField("nodes.projects"));
        }
        validate_element_provenance(&node.provenance, &pending_ids)?;
    }
    for edge in &artifact.edges {
        if !key_set.contains(&edge.blocked) {
            return Err(GraphError::DanglingEndpoint(edge.blocked.to_string()));
        }
        if !key_set.contains(&edge.blocker) {
            return Err(GraphError::DanglingEndpoint(edge.blocker.to_string()));
        }
        validate_element_provenance(&edge.provenance, &pending_ids)?;
    }
    if !artifact.edges.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(GraphError::NonDeterministicEdgeOrder);
    }
    let expected = calculate_hash(artifact)?;
    if artifact.artifact_hash != expected {
        return Err(GraphError::ArtifactHashMismatch);
    }
    Ok(())
}

fn valid_projects(node: &ArtifactNode) -> bool {
    let Some(projects) = &node.projects else {
        return true;
    };
    node.kind == NodeKind::Issue
        && !projects.is_empty()
        && projects.iter().all(|project| !project.trim().is_empty())
        && projects
            .windows(2)
            .all(|pair| pair[0].to_ascii_lowercase() < pair[1].to_ascii_lowercase())
}

fn validate_element_provenance(
    provenance: &ElementProvenance,
    pending_ids: &BTreeSet<&str>,
) -> Result<(), GraphError> {
    validate_provenance(provenance.state, &provenance.operation_ids)?;
    if provenance
        .operation_ids
        .iter()
        .any(|operation| !pending_ids.contains(operation.as_str()))
    {
        return Err(GraphError::InvalidProvenance);
    }
    Ok(())
}

fn validate_provenance(state: ProvenanceState, operation_ids: &[String]) -> Result<(), GraphError> {
    let state_matches_operations = match state {
        ProvenanceState::Synchronized => operation_ids.is_empty(),
        ProvenanceState::Pending => !operation_ids.is_empty(),
    };
    if operation_ids.iter().any(String::is_empty)
        || !operation_ids.windows(2).all(|pair| pair[0] < pair[1])
        || !state_matches_operations
    {
        return Err(GraphError::InvalidProvenance);
    }
    Ok(())
}

#[derive(Serialize)]
struct ArtifactHashInput<'a> {
    schema_version: ArtifactSchemaVersion,
    repository: &'a str,
    effective_input_hash: &'a str,
    provenance: &'a ArtifactProvenance,
    operational_counts: &'a OperationalCounts,
    nodes: &'a [ArtifactNode],
    edges: &'a [ArtifactEdge],
}

fn calculate_hash(artifact: &GraphArtifact) -> Result<String, GraphError> {
    let input = ArtifactHashInput {
        schema_version: artifact.schema_version,
        repository: &artifact.repository,
        effective_input_hash: &artifact.effective_input_hash,
        provenance: &artifact.provenance,
        operational_counts: &artifact.operational_counts,
        nodes: &artifact.nodes,
        edges: &artifact.edges,
    };
    let bytes = serde_json::to_vec(&input).map_err(GraphError::EncodeArtifact)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn normalized_state(value: &str) -> &'static str {
    if value.eq_ignore_ascii_case("open") {
        "open"
    } else if value.eq_ignore_ascii_case("closed") {
        "closed"
    } else {
        "unknown"
    }
}

fn unresolved_position() -> Position {
    Position {
        layer: None,
        x: 0,
        y: 0,
    }
}

fn sort_and_deduplicate(values: &mut Vec<String>) {
    values.sort_by_key(|value| value.to_ascii_lowercase());
    values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
}

fn strip_operation_markers(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_project_membership_is_valid_and_rendered_only_when_present() {
        let synchronized = ElementProvenance::synchronized();
        let mut artifact = GraphArtifact {
            schema_version: ArtifactSchemaVersion::V1,
            schema_url: SchemaLocation::Local,
            repository: "acme/widgets".to_owned(),
            synced_at: "2026-08-07T00:00:00Z".to_owned(),
            input_hash: "a".repeat(64),
            effective_input_hash: "a".repeat(64),
            artifact_hash: String::new(),
            provenance: ArtifactProvenance {
                base: ProvenanceState::Synchronized,
                state: ProvenanceState::Synchronized,
                pending_mutation_count: 0,
                pending_operation_ids: Vec::new(),
            },
            operational_counts: OperationalCounts {
                operational_issue_count: 1,
                ready_count: 1,
                executable_count: 1,
                assigned_ready_count: 0,
                blocked_count: 0,
            },
            nodes: vec![ArtifactNode {
                key: NodeKey::new("acme/widgets", 1),
                repository: "acme/widgets".to_owned(),
                number: 1,
                kind: NodeKind::Issue,
                url: Some("https://github.com/acme/widgets/issues/1".to_owned()),
                title: Some("Project work".to_owned()),
                state: "open".to_owned(),
                readiness: Readiness::Ready,
                assignees: Vec::new(),
                labels: Vec::new(),
                projects: Some(vec!["Platform".to_owned(), "Roadmap".to_owned()]),
                position: Position {
                    layer: Some(0),
                    x: 0,
                    y: 0,
                },
                provenance: synchronized,
            }],
            edges: Vec::new(),
        };
        artifact.artifact_hash = calculate_hash(&artifact).expect("artifact hash");

        validate(&artifact).expect("valid artifact with Project membership");
        let serialized = serde_json::to_vec(&artifact).expect("serialized artifact");
        validate_serialized(&serialized).expect("serialized Project artifact");
        let html =
            crate::graph::render::html(&artifact, &crate::graph::presentation::build(&artifact))
                .expect("rendered Project artifact");
        assert!(html.contains("<th scope=\"col\">Projects</th>"));
        assert!(html.contains("<td>Platform, Roadmap</td>"));

        artifact.nodes[0].projects = None;
        artifact.artifact_hash = calculate_hash(&artifact).expect("artifact hash without Projects");
        validate(&artifact).expect("valid artifact without Project membership");
        let html =
            crate::graph::render::html(&artifact, &crate::graph::presentation::build(&artifact))
                .expect("rendered artifact without Projects");
        assert!(!html.contains("<th scope=\"col\">Projects</th>"));
    }
}
