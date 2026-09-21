use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    GraphError,
    model::{NodeKey, Position, unresolved_position},
    text::sort_and_deduplicate,
};
use crate::{
    model::{BlockerScope, strip_operation_markers},
    operational::{ExecutionScope, PreparedRepository},
    plan::{DependencyLayers, PlanIssue},
    priority::PriorityState,
    ranking::{self, NextAnalysis, PlanDecision},
    working_graph::{PendingProvenance, WorkingGraph},
};

pub(crate) const ARTIFACT_SCHEMA_VERSION: &str = "grit.graph-artifact/v2";

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum ArtifactSchemaVersion {
    #[serde(rename = "grit.graph-artifact/v2")]
    V2,
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
    analysis: GraphAnalysis,
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
pub(super) struct ElementProvenance {
    state: ProvenanceState,
    operation_ids: Vec<String>,
}

impl ElementProvenance {
    fn pending(provenance: PendingProvenance) -> Self {
        let mut operation_ids = provenance.operation_ids().to_vec();
        operation_ids.sort();
        operation_ids.dedup();
        Self {
            state: if provenance.is_pending() {
                ProvenanceState::Pending
            } else {
                ProvenanceState::Synchronized
            },
            operation_ids,
        }
    }

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
struct GraphAnalysis {
    policy_version: AnalysisPolicyVersion,
    execution_scope: ArtifactExecutionScope,
    next: NextAnalysis,
    plan: ArtifactPlan,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
enum AnalysisPolicyVersion {
    #[serde(rename = "next/v1")]
    NextV1,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum ArtifactExecutionScope {
    Available,
    Assignee { assignee: String },
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct ArtifactPlan {
    decision: PlanDecision,
    parallel_now: Vec<PlanIssue>,
    dependency_layers: DependencyLayers,
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "node")]
pub(super) enum ArtifactNode {
    Issue {
        common: NodeCommon,
        status: IssueNodeStatus,
        url: String,
        title: String,
        assignees: Vec<String>,
        labels: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projects: Option<Vec<String>>,
        priority: PriorityState,
        #[serde(skip_serializing_if = "Option::is_none")]
        pagerank_bucket: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        unlock_count: Option<usize>,
    },
    ExternalBlocker {
        common: NodeCommon,
        status: ExternalNodeStatus,
    },
}

#[derive(Clone, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NodeCommon {
    #[schemars(with = "String")]
    key: NodeKey,
    repository: String,
    number: u64,
    position: Position,
    provenance: ElementProvenance,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum IssueNodeStatus {
    Ready,
    Blocked,
    Closed,
    Unknown,
}

#[derive(Clone, Copy, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExternalNodeStatus {
    #[serde(rename = "external_open")]
    Open,
    #[serde(rename = "external_closed")]
    Closed,
    #[serde(rename = "external_unknown")]
    Unknown,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum LayerRole {
    Satisfied,
    OpenIssue,
    Opaque,
}

impl ArtifactNode {
    pub(super) fn projects(&self) -> Option<&[String]> {
        match self {
            Self::Issue { projects, .. } => projects.as_deref(),
            Self::ExternalBlocker { .. } => None,
        }
    }

    pub(super) fn key(&self) -> &NodeKey {
        &self.common().key
    }

    fn repository(&self) -> &str {
        &self.common().repository
    }

    fn number(&self) -> u64 {
        self.common().number
    }

    pub(super) fn common(&self) -> &NodeCommon {
        match self {
            Self::Issue { common, .. } | Self::ExternalBlocker { common, .. } => common,
        }
    }

    fn common_mut(&mut self) -> &mut NodeCommon {
        match self {
            Self::Issue { common, .. } | Self::ExternalBlocker { common, .. } => common,
        }
    }

    pub(super) fn is_open_issue(&self) -> bool {
        match self {
            Self::Issue { status, .. } => {
                matches!(status, IssueNodeStatus::Ready | IssueNodeStatus::Blocked)
            }
            Self::ExternalBlocker { .. } => false,
        }
    }

    pub(super) fn layer_role(&self) -> LayerRole {
        match self {
            Self::Issue {
                status: IssueNodeStatus::Closed,
                ..
            }
            | Self::ExternalBlocker {
                status: ExternalNodeStatus::Closed,
                ..
            } => LayerRole::Satisfied,
            Self::Issue {
                status: IssueNodeStatus::Ready | IssueNodeStatus::Blocked,
                ..
            } => LayerRole::OpenIssue,
            Self::Issue {
                status: IssueNodeStatus::Unknown,
                ..
            }
            | Self::ExternalBlocker { .. } => LayerRole::Opaque,
        }
    }

    pub(super) fn lifecycle(&self) -> &'static str {
        match self {
            Self::Issue {
                status: IssueNodeStatus::Ready | IssueNodeStatus::Blocked,
                ..
            }
            | Self::ExternalBlocker {
                status: ExternalNodeStatus::Open,
                ..
            } => "open",
            Self::Issue {
                status: IssueNodeStatus::Closed,
                ..
            }
            | Self::ExternalBlocker {
                status: ExternalNodeStatus::Closed,
                ..
            } => "closed",
            Self::Issue {
                status: IssueNodeStatus::Unknown,
                ..
            }
            | Self::ExternalBlocker {
                status: ExternalNodeStatus::Unknown,
                ..
            } => "unknown",
        }
    }

    pub(super) fn status(&self) -> &'static str {
        match self {
            Self::Issue { status, .. } => status.as_str(),
            Self::ExternalBlocker { status, .. } => status.as_str(),
        }
    }

    pub(super) fn position(&self) -> &Position {
        &self.common().position
    }

    pub(super) fn position_mut(&mut self) -> &mut Position {
        &mut self.common_mut().position
    }

    fn provenance(&self) -> &ElementProvenance {
        &self.common().provenance
    }
}

impl IssueNodeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

impl ExternalNodeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "external_open",
            Self::Closed => "external_closed",
            Self::Unknown => "external_unknown",
        }
    }
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

#[cfg(test)]
use crate::model::LocalReplica;

#[cfg(test)]
pub(super) fn build(
    replica: &LocalReplica,
    scope: ExecutionScope<'_>,
    horizon: u8,
) -> Result<GraphArtifact, GraphError> {
    let outbox = serde_json::from_value(serde_json::json!({
        "schema_version": "grit.pending-mutations/v1",
        "repository": replica.repository,
        "operations": [],
    }))
    .expect("empty fixture outbox");
    let working = WorkingGraph::project(replica, &outbox).expect("synchronized fixture");
    build_working(&working, scope, horizon)
}

pub(super) fn build_working(
    working: &WorkingGraph<'_>,
    scope: ExecutionScope<'_>,
    horizon: u8,
) -> Result<GraphArtifact, GraphError> {
    let replica = working.replica();
    let prepared = PreparedRepository::prepare(working);
    let ranking::AnalysisBundle {
        run,
        ready,
        candidate_unlock_counts: unlock_counts,
        pagerank_buckets,
    } = ranking::analyze_prepared_bundle(
        &prepared,
        scope,
        horizon,
        &mut ranking::RankingCache::default(),
    );
    let next = run.analysis;
    let ready_numbers: BTreeSet<_> = ready.ready.iter().map(|issue| issue.number).collect();
    let effective_input_hash = next.input_hash().to_owned();
    let decision = next.clone().into_plan_decision();
    let structural = crate::plan::analyze_with_ready(&prepared, scope, &ready);
    let execution_scope = match scope {
        ExecutionScope::Available => ArtifactExecutionScope::Available,
        ExecutionScope::Assignee(assignee) => ArtifactExecutionScope::Assignee {
            assignee: assignee.to_owned(),
        },
    };
    let analysis = GraphAnalysis {
        policy_version: AnalysisPolicyVersion::NextV1,
        execution_scope,
        next,
        plan: ArtifactPlan {
            decision,
            parallel_now: structural.parallel_now,
            dependency_layers: structural.dependency_layers,
        },
    };
    let mut nodes = Vec::new();
    let mut node_keys = BTreeSet::new();

    for issue in &replica.issues {
        let key = NodeKey::new(&replica.repository, issue.number);
        if !node_keys.insert(key.clone()) {
            return Err(GraphError::DuplicateNode(key.to_string()));
        }
        let state = normalized_state(&issue.state);
        let status = if state == "closed" {
            IssueNodeStatus::Closed
        } else if state != "open" {
            IssueNodeStatus::Unknown
        } else if ready_numbers.contains(&issue.number) {
            IssueNodeStatus::Ready
        } else {
            IssueNodeStatus::Blocked
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
        let unlock_count = unlock_counts.get(&issue.number).copied();
        nodes.push(ArtifactNode::Issue {
            common: NodeCommon {
                key,
                repository: replica.repository.clone(),
                number: issue.number,
                position: unresolved_position(),
                provenance: ElementProvenance::pending(working.provenance_for_issue(issue.number)),
            },
            status,
            url: issue.url.clone(),
            title: strip_operation_markers(&issue.title),
            assignees,
            labels,
            priority: working.priority(issue),
            pagerank_bucket: pagerank_buckets.get(&issue.number).copied(),
            unlock_count,
            projects: None,
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
        nodes.push(ArtifactNode::ExternalBlocker {
            common: NodeCommon {
                repository: key.repository().to_owned(),
                number: key.number(),
                key: key.clone(),
                position: unresolved_position(),
                provenance: ElementProvenance::synchronized(),
            },
            status: match state.as_str() {
                "open" => ExternalNodeStatus::Open,
                "closed" => ExternalNodeStatus::Closed,
                _ => ExternalNodeStatus::Unknown,
            },
        });
        node_keys.insert(key);
    }

    nodes.sort_by(|left, right| left.key().cmp(right.key()));
    let edges: Vec<_> = edges.into_iter().collect();
    super::layout::assign_artifact_dependency_layers(&mut nodes, &edges)?;
    let mut pending_operation_ids = working.operation_ids();
    pending_operation_ids.sort();
    let provenance = ArtifactProvenance {
        base: ProvenanceState::Synchronized,
        state: if working.is_pending() {
            ProvenanceState::Pending
        } else {
            ProvenanceState::Synchronized
        },
        pending_mutation_count: pending_operation_ids.len() as u64,
        pending_operation_ids,
    };
    let operational_counts = OperationalCounts {
        operational_issue_count: ready.operational_issue_count,
        ready_count: ready.ready_count,
        executable_count: ready.executable.len(),
        assigned_ready_count: ready.assigned_ready_count,
        blocked_count: ready.blocked_count,
    };
    let mut artifact = GraphArtifact {
        schema_version: ArtifactSchemaVersion::V2,
        schema_url: SchemaLocation::Local,
        repository: replica.repository.clone(),
        synced_at: replica.synced_at.clone(),
        input_hash: replica.input_hash.clone(),
        effective_input_hash,
        artifact_hash: String::new(),
        provenance,
        operational_counts,
        analysis,
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
    if artifact.schema_version != ArtifactSchemaVersion::V2
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
    if artifact.analysis.policy_version != AnalysisPolicyVersion::NextV1
        || artifact.analysis.next.input_hash() != artifact.effective_input_hash
        || artifact.analysis.plan.decision.input_hash() != artifact.effective_input_hash
    {
        return Err(GraphError::InvalidField("analysis"));
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
        .all(|pair| pair[0].key() < pair[1].key())
    {
        return Err(GraphError::NonDeterministicNodeOrder);
    }
    let key_set: BTreeSet<_> = artifact.nodes.iter().map(ArtifactNode::key).collect();
    for node in &artifact.nodes {
        if node.number() != node.key().number() || node.repository() != node.key().repository() {
            return Err(GraphError::InvalidStableKey(node.key().to_string()));
        }
        match node {
            ArtifactNode::Issue { common, .. }
                if !common.repository.eq_ignore_ascii_case(&artifact.repository) =>
            {
                return Err(GraphError::InvalidField("nodes"));
            }
            ArtifactNode::Issue { .. } | ArtifactNode::ExternalBlocker { .. } => {}
        }
        if !valid_projects(node) {
            return Err(GraphError::InvalidField("nodes.projects"));
        }
        validate_element_provenance(node.provenance(), &pending_ids)?;
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
    let Some(projects) = node.projects() else {
        return true;
    };
    !projects.is_empty()
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
    analysis: &'a GraphAnalysis,
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
        analysis: &artifact.analysis,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_project_membership_is_valid_and_rendered_only_when_present() {
        let issue = crate::model::Issue {
            id: 1,
            node_id: "I_1".to_owned(),
            number: 1,
            url: "https://github.com/acme/widgets/issues/1".to_owned(),
            title: "Project work".to_owned(),
            body: String::new(),
            state: "open".to_owned(),
            state_reason: None,
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            created_at: "2026-08-01T00:00:00Z".to_owned(),
            updated_at: "2026-08-01T00:00:00Z".to_owned(),
            closed_at: None,
        };
        let replica = LocalReplica::build(
            "acme/widgets".to_owned(),
            "2026-08-07T00:00:00Z".to_owned(),
            Vec::new(),
            vec![issue],
            Vec::new(),
        )
        .expect("Project fixture");
        let mut artifact = build(
            &replica,
            ExecutionScope::Available,
            ranking::DEFAULT_HORIZON,
        )
        .expect("Project artifact");
        if let ArtifactNode::Issue { projects, .. } = &mut artifact.nodes[0] {
            *projects = Some(vec!["Platform".to_owned(), "Roadmap".to_owned()]);
        }
        artifact.artifact_hash = calculate_hash(&artifact).expect("artifact hash");
        validate(&artifact).expect("valid artifact with Project membership");
        validate_serialized(&serde_json::to_vec(&artifact).expect("serialized artifact"))
            .expect("serialized Project artifact");
        let html =
            crate::graph::render::html(&artifact, &crate::graph::presentation::build(&artifact))
                .expect("rendered Project artifact");
        assert!(html.contains("<th scope=\"col\">Projects</th>"));
        assert!(html.contains("<td>Platform, Roadmap</td>"));

        if let ArtifactNode::Issue { projects, .. } = &mut artifact.nodes[0] {
            *projects = None;
        }
        artifact.artifact_hash = calculate_hash(&artifact).expect("artifact hash without Projects");
        validate(&artifact).expect("valid artifact without Project membership");
        let html =
            crate::graph::render::html(&artifact, &crate::graph::presentation::build(&artifact))
                .expect("rendered artifact without Projects");
        assert!(!html.contains("<th scope=\"col\">Projects</th>"));
    }
}
