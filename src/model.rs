use serde::{Deserialize, Serialize};

pub(crate) const REPLICA_SCHEMA_VERSION: &str = "grit.local-replica/v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LocalReplica {
    pub(crate) schema_version: &'static str,
    pub(crate) repository: String,
    pub(crate) synced_at: String,
    pub(crate) input_hash: String,
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
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
