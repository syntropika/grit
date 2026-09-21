use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    atomic_file::{self, AtomicFileError},
    draft_identity::DraftIdentity,
    model::{DependencyEdgeKey, DependencyPresence, TemporaryIssueId},
    priority::LogicalPriority,
    repository::Repository,
    store::{StoreError, repository_state_directory},
};

pub(crate) const OUTBOX_SCHEMA_VERSION: &str = "grit.pending-mutations/v1";
static OPERATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PendingMutationOutbox {
    schema_version: String,
    repository: String,
    operations: Vec<PendingMutation>,
}

impl PendingMutationOutbox {
    fn empty(repository: &Repository) -> Self {
        Self {
            schema_version: OUTBOX_SCHEMA_VERSION.to_owned(),
            repository: repository.full_name().to_owned(),
            operations: Vec::new(),
        }
    }

    fn validate(&self, repository: &Repository) -> Result<(), OutboxError> {
        if self.schema_version != OUTBOX_SCHEMA_VERSION {
            return Err(OutboxError::UnsupportedSchema(self.schema_version.clone()));
        }
        if !self.repository.eq_ignore_ascii_case(repository.full_name()) {
            return Err(OutboxError::RepositoryMismatch {
                expected: repository.full_name().to_owned(),
                actual: self.repository.clone(),
            });
        }
        let mut operation_ids = BTreeSet::new();
        let mut temporary_ids = BTreeSet::new();
        let mut synthetic_numbers = BTreeSet::new();
        for operation in &self.operations {
            if operation.affected_issue_numbers().contains(&0) {
                return Err(OutboxError::InvalidIssueNumber);
            }
            if operation.depends_on().iter().any(|dependency| {
                dependency == operation.id() || !operation_ids.contains(dependency.as_str())
            }) {
                return Err(OutboxError::InvalidDependency(operation.id().to_owned()));
            }
            if !operation_ids.insert(operation.id().to_owned()) {
                return Err(OutboxError::DuplicateOperationId(operation.id().to_owned()));
            }
            if let Some(create) = operation.issue_create_view()
                && (create.synthetic_number != create.temporary_id.synthetic_number()
                    || create.title.trim().is_empty()
                    || !operation.depends_on().is_empty()
                    || uuid::Uuid::parse_str(create.marker).is_err()
                    || operation
                        .id()
                        .strip_prefix("op-")
                        .is_none_or(|value| uuid::Uuid::parse_str(value).is_err())
                    || !temporary_ids.insert(create.temporary_id)
                    || !synthetic_numbers.insert(create.synthetic_number))
            {
                return Err(OutboxError::InvalidDraftIssue(operation.id().to_owned()));
            }
            if operation
                .priority_values()
                .is_some_and(|(_, desired)| matches!(desired, LogicalPriority::Conflict { .. }))
            {
                return Err(OutboxError::InvalidDesiredPriority);
            }
            if let Some((edge, _)) = operation.dependency_values() {
                let repositories_valid = Repository::parse(edge.blocked_repository()).is_ok()
                    && Repository::parse(edge.blocker_repository()).is_ok();
                let blocked_matches = edge
                    .blocked_repository()
                    .eq_ignore_ascii_case(repository.full_name());
                let self_dependency = edge
                    .blocked_repository()
                    .eq_ignore_ascii_case(edge.blocker_repository())
                    && edge.blocked_number() == edge.blocker_number();
                if !repositories_valid || !blocked_matches || self_dependency {
                    return Err(OutboxError::InvalidDependencyEdge(
                        operation.id().to_owned(),
                    ));
                }
            }
            if !operation.has_safe_write_plan() {
                return Err(OutboxError::InvalidWritePlan(operation.id().to_owned()));
            }
        }
        Ok(())
    }

    pub(crate) fn operations(&self) -> &[PendingMutation] {
        &self.operations
    }

    pub(crate) fn latest_priority_operation_for_issue(&self, issue_number: u64) -> Option<&str> {
        self.operations
            .iter()
            .rev()
            .find(|operation| {
                operation.priority_values().is_some() && operation.issue_number() == issue_number
            })
            .map(PendingMutation::id)
    }

    pub(crate) fn latest_operation_for_dependency(&self, edge: &DependencyEdgeKey) -> Option<&str> {
        self.operations
            .iter()
            .rev()
            .find(|operation| {
                operation
                    .dependency_values()
                    .is_some_and(|(candidate, _)| candidate == edge)
            })
            .map(PendingMutation::id)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PendingMutation {
    #[serde(flatten)]
    header: MutationHeader,
    #[serde(flatten)]
    payload: MutationPayload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MutationHeader {
    id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MutationPayload {
    IssueCreate {
        temporary_id: TemporaryIssueId,
        synthetic_number: u64,
        title: String,
        body: String,
        created_at: String,
        marker: String,
        #[serde(default, skip_serializing_if = "IssueCreateState::is_pending")]
        state: IssueCreateState,
    },
    PriorityUpdate {
        issue_number: u64,
        base: LogicalPriority,
        desired: LogicalPriority,
        #[serde(default, skip_serializing_if = "PriorityMutationState::is_pending")]
        state: PriorityMutationState,
    },
    DependencyUpdate {
        edge: DependencyEdgeKey,
        desired: DependencyPresence,
        #[serde(default, skip_serializing_if = "DependencyMutationState::is_pending")]
        state: DependencyMutationState,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationKind {
    IssueCreate,
    PriorityUpdate,
    DependencyUpdate,
}

pub(crate) struct IssueCreateView<'a> {
    pub(crate) temporary_id: TemporaryIssueId,
    pub(crate) synthetic_number: u64,
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
    pub(crate) created_at: &'a str,
    pub(crate) marker: &'a str,
    pub(crate) state: &'a IssueCreateState,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum IssueCreateState {
    #[default]
    Pending,
    AwaitingMarker {
        error: String,
    },
    Mapped {
        issue_id: u64,
        issue_node_id: String,
        issue_number: u64,
        issue_url: String,
    },
}

impl IssueCreateState {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    fn permits_dependents(&self) -> bool {
        matches!(self, Self::Mapped { .. })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum PriorityMutationState {
    #[default]
    Pending,
    Conflicting {
        remote: LogicalPriority,
    },
    Applying {
        expected_labels: Vec<String>,
        remaining_writes: Vec<PriorityWrite>,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_error: Option<String>,
    },
    Applied,
    AlreadySatisfied,
    ResolvedRemote {
        remote: LogicalPriority,
    },
    Failed {
        error: String,
    },
    TransitivelyBlocked {
        blocked_by: Vec<String>,
    },
}

impl PriorityMutationState {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    pub(crate) fn is_successfully_terminal(&self) -> bool {
        matches!(
            self,
            Self::Applied | Self::AlreadySatisfied | Self::ResolvedRemote { .. }
        )
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum DependencyMutationState {
    #[default]
    Pending,
    Applying {
        #[serde(skip_serializing_if = "Option::is_none")]
        last_error: Option<String>,
    },
    Applied,
    AlreadySatisfied,
    Failed {
        error: String,
    },
    TransitivelyBlocked {
        blocked_by: Vec<String>,
    },
}

impl DependencyMutationState {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    fn is_successfully_terminal(&self) -> bool {
        matches!(self, Self::Applied | Self::AlreadySatisfied)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum PriorityWrite {
    Add { label: String },
    Remove { label: String },
}

impl PriorityWrite {
    pub(crate) fn apply_to(&self, labels: &mut Vec<String>) {
        match self {
            Self::Add { label } => {
                if !labels.iter().any(|candidate| candidate == label) {
                    labels.push(label.clone());
                    labels.sort();
                }
            }
            Self::Remove { label } => labels.retain(|candidate| candidate != label),
        }
    }

    pub(crate) fn canonical_plan(
        current: &[String],
        desired: &LogicalPriority,
    ) -> Option<Vec<Self>> {
        LogicalPriority::from_canonical_labels(current)?;
        let desired_label = match desired {
            LogicalPriority::Declared { value } => Some(value.canonical_label()),
            LogicalPriority::Unspecified => None,
            LogicalPriority::Conflict { .. } => return None,
        };
        let mut writes = Vec::new();
        if let Some(desired_label) = desired_label
            && !current.iter().any(|label| label == desired_label)
        {
            writes.push(Self::Add {
                label: desired_label.to_owned(),
            });
        }
        for label in current {
            if Some(label.as_str()) != desired_label {
                writes.push(Self::Remove {
                    label: label.clone(),
                });
            }
        }
        Some(writes)
    }
}

impl PendingMutation {
    pub(crate) fn issue_create(
        temporary_id: TemporaryIssueId,
        title: String,
        body: String,
    ) -> Self {
        let synthetic_number = temporary_id.synthetic_number();
        let id = format!("op-{}", uuid::Uuid::new_v4());
        Self {
            header: MutationHeader {
                id,
                depends_on: Vec::new(),
            },
            payload: MutationPayload::IssueCreate {
                marker: uuid::Uuid::new_v4().to_string(),
                temporary_id,
                synthetic_number,
                title,
                body,
                created_at: chrono::Utc::now().to_rfc3339(),
                state: IssueCreateState::Pending,
            },
        }
    }

    pub(crate) fn priority_update(
        repository: &Repository,
        issue_number: u64,
        base: LogicalPriority,
        desired: LogicalPriority,
        depends_on: Vec<String>,
    ) -> Self {
        Self {
            header: MutationHeader {
                id: new_operation_id(repository, issue_number),
                depends_on,
            },
            payload: MutationPayload::PriorityUpdate {
                issue_number,
                base,
                desired,
                state: PriorityMutationState::Pending,
            },
        }
    }

    pub(crate) fn dependency_update(
        repository: &Repository,
        edge: DependencyEdgeKey,
        desired: DependencyPresence,
        depends_on: Vec<String>,
    ) -> Self {
        Self {
            header: MutationHeader {
                id: new_operation_id(repository, edge.blocked_number()),
                depends_on,
            },
            payload: MutationPayload::DependencyUpdate {
                edge,
                desired,
                state: DependencyMutationState::Pending,
            },
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.header.id
    }

    pub(crate) fn kind(&self) -> MutationKind {
        match self.payload {
            MutationPayload::IssueCreate { .. } => MutationKind::IssueCreate,
            MutationPayload::PriorityUpdate { .. } => MutationKind::PriorityUpdate,
            MutationPayload::DependencyUpdate { .. } => MutationKind::DependencyUpdate,
        }
    }

    pub(crate) fn issue_number(&self) -> u64 {
        match &self.payload {
            MutationPayload::IssueCreate {
                synthetic_number,
                state,
                ..
            } => match state {
                IssueCreateState::Mapped { issue_number, .. } => *issue_number,
                _ => *synthetic_number,
            },
            MutationPayload::PriorityUpdate { issue_number, .. } => *issue_number,
            MutationPayload::DependencyUpdate { edge, .. } => edge.blocked_number(),
        }
    }

    pub(crate) fn affected_issue_numbers(&self) -> Vec<u64> {
        match &self.payload {
            MutationPayload::IssueCreate { .. } => vec![self.issue_number()],
            MutationPayload::PriorityUpdate { issue_number, .. } => vec![*issue_number],
            MutationPayload::DependencyUpdate { edge, .. } => {
                let mut affected = vec![edge.blocked_number()];
                if edge.is_internal() {
                    affected.push(edge.blocker_number());
                }
                affected
            }
        }
    }

    pub(crate) fn priority_values(&self) -> Option<(&LogicalPriority, &LogicalPriority)> {
        match &self.payload {
            MutationPayload::PriorityUpdate { base, desired, .. } => Some((base, desired)),
            MutationPayload::IssueCreate { .. } | MutationPayload::DependencyUpdate { .. } => None,
        }
    }

    pub(crate) fn dependency_values(&self) -> Option<(&DependencyEdgeKey, DependencyPresence)> {
        match &self.payload {
            MutationPayload::DependencyUpdate { edge, desired, .. } => Some((edge, *desired)),
            MutationPayload::IssueCreate { .. } | MutationPayload::PriorityUpdate { .. } => None,
        }
    }

    pub(crate) fn depends_on(&self) -> &[String] {
        &self.header.depends_on
    }

    pub(crate) fn priority_state(&self) -> Option<&PriorityMutationState> {
        match &self.payload {
            MutationPayload::PriorityUpdate { state, .. } => Some(state),
            MutationPayload::IssueCreate { .. } | MutationPayload::DependencyUpdate { .. } => None,
        }
    }

    pub(crate) fn dependency_state(&self) -> Option<&DependencyMutationState> {
        match &self.payload {
            MutationPayload::DependencyUpdate { state, .. } => Some(state),
            MutationPayload::IssueCreate { .. } | MutationPayload::PriorityUpdate { .. } => None,
        }
    }

    pub(crate) fn issue_create_state(&self) -> Option<&IssueCreateState> {
        match &self.payload {
            MutationPayload::IssueCreate { state, .. } => Some(state),
            MutationPayload::PriorityUpdate { .. } | MutationPayload::DependencyUpdate { .. } => {
                None
            }
        }
    }

    pub(crate) fn issue_create_view(&self) -> Option<IssueCreateView<'_>> {
        match &self.payload {
            MutationPayload::IssueCreate {
                temporary_id,
                synthetic_number,
                title,
                body,
                created_at,
                marker,
                state,
            } => Some(IssueCreateView {
                temporary_id: *temporary_id,
                synthetic_number: *synthetic_number,
                title,
                body,
                created_at,
                marker,
                state,
            }),
            MutationPayload::PriorityUpdate { .. } | MutationPayload::DependencyUpdate { .. } => {
                None
            }
        }
    }

    pub(crate) fn permits_dependents(&self) -> bool {
        match &self.payload {
            MutationPayload::IssueCreate { state, .. } => state.permits_dependents(),
            MutationPayload::PriorityUpdate { state, .. } => state.is_successfully_terminal(),
            MutationPayload::DependencyUpdate { state, .. } => state.is_successfully_terminal(),
        }
    }

    pub(crate) fn is_successfully_terminal(&self) -> bool {
        self.permits_dependents()
    }

    pub(crate) fn is_pending_intent(&self) -> bool {
        !self.is_successfully_terminal()
    }

    pub(crate) fn effective_priority(&self) -> Option<&LogicalPriority> {
        match &self.payload {
            MutationPayload::PriorityUpdate { desired, state, .. } => match state {
                PriorityMutationState::ResolvedRemote { remote } => Some(remote),
                _ => Some(desired),
            },
            MutationPayload::IssueCreate { .. } | MutationPayload::DependencyUpdate { .. } => None,
        }
    }

    fn has_safe_write_plan(&self) -> bool {
        match &self.payload {
            MutationPayload::IssueCreate { .. } => true,
            MutationPayload::PriorityUpdate { desired, state, .. } => {
                let PriorityMutationState::Applying {
                    expected_labels,
                    remaining_writes,
                    ..
                } = state
                else {
                    return true;
                };
                !remaining_writes.is_empty()
                    && PriorityWrite::canonical_plan(expected_labels, desired)
                        .is_some_and(|canonical| canonical == *remaining_writes)
            }
            MutationPayload::DependencyUpdate { .. } => true,
        }
    }

    fn rebase_priority(
        &mut self,
        base: LogicalPriority,
        desired: Option<LogicalPriority>,
    ) -> Result<(), ()> {
        match &mut self.payload {
            MutationPayload::IssueCreate { .. } => Err(()),
            MutationPayload::PriorityUpdate {
                base: current_base,
                desired: current_desired,
                state,
                ..
            } => {
                *current_base = base;
                if let Some(desired) = desired {
                    *current_desired = desired;
                }
                *state = PriorityMutationState::Pending;
                Ok(())
            }
            MutationPayload::DependencyUpdate { .. } => Err(()),
        }
    }
}

pub(crate) struct OutboxStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl OutboxStore {
    pub(crate) fn discover(repository: &Repository) -> Result<Self, OutboxError> {
        let directory =
            repository_state_directory(repository).map_err(OutboxError::StateDirectory)?;
        Ok(Self {
            path: directory.join("outbox.json"),
            lock_path: directory.join("outbox.lock"),
        })
    }

    pub(crate) fn load(
        &self,
        repository: &Repository,
    ) -> Result<PendingMutationOutbox, OutboxError> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PendingMutationOutbox::empty(repository));
            }
            Err(error) => return Err(OutboxError::Read(error)),
        };
        let outbox: PendingMutationOutbox =
            serde_json::from_reader(file).map_err(OutboxError::Decode)?;
        outbox.validate(repository)?;
        Ok(outbox)
    }

    pub(crate) fn begin_transaction(
        &self,
        repository: &Repository,
    ) -> Result<OutboxTransaction<'_>, OutboxError> {
        let parent = self.path.parent().expect("outbox path always has a parent");
        fs::create_dir_all(parent).map_err(OutboxError::CreateDirectory)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&self.lock_path)
            .map_err(OutboxError::OpenLock)?;
        FileExt::lock_exclusive(&lock).map_err(OutboxError::Lock)?;
        let outbox = self.load(repository)?;
        let operation_index = operation_index(&outbox);
        Ok(OutboxTransaction {
            store: self,
            _lock: lock,
            outbox,
            operation_index,
        })
    }

    fn publish(&self, outbox: &PendingMutationOutbox) -> Result<(), OutboxError> {
        let mut bytes = serde_json::to_vec_pretty(outbox).map_err(OutboxError::Encode)?;
        bytes.push(b'\n');
        atomic_file::publish(&self.path, "outbox", &bytes).map_err(OutboxError::Publish)
    }
}

pub(crate) struct OutboxTransaction<'a> {
    store: &'a OutboxStore,
    _lock: File,
    outbox: PendingMutationOutbox,
    operation_index: BTreeMap<String, usize>,
}

pub(crate) enum MutationStateUpdate {
    Priority(PriorityMutationState),
    Dependency(DependencyMutationState),
}

impl OutboxTransaction<'_> {
    pub(crate) fn outbox(&self) -> &PendingMutationOutbox {
        &self.outbox
    }

    pub(crate) fn append(
        mut self,
        repository: &Repository,
        operation: PendingMutation,
    ) -> Result<PendingMutationOutbox, OutboxError> {
        self.outbox.operations.push(operation);
        self.outbox.validate(repository)?;
        self.store.publish(&self.outbox)?;
        Ok(self.outbox)
    }

    pub(crate) fn operations(&self) -> &[PendingMutation] {
        self.outbox.operations()
    }

    pub(crate) fn operation(&self, id: &str) -> Option<&PendingMutation> {
        self.operation_index
            .get(id)
            .map(|index| &self.outbox.operations[*index])
    }

    pub(crate) fn stage_priority_state(
        &mut self,
        id: &str,
        state: PriorityMutationState,
    ) -> Result<(), OutboxError> {
        match &mut self.operation_mut(id)?.payload {
            MutationPayload::PriorityUpdate { state: current, .. } => {
                *current = state;
                Ok(())
            }
            MutationPayload::IssueCreate { .. } | MutationPayload::DependencyUpdate { .. } => {
                Err(OutboxError::InvalidMutationKind(id.to_owned()))
            }
        }
    }

    pub(crate) fn checkpoint_issue_create_state(
        &mut self,
        repository: &Repository,
        id: &str,
        state: IssueCreateState,
    ) -> Result<(), OutboxError> {
        self.stage_issue_create_state(id, state)?;
        self.publish(repository)
    }

    pub(crate) fn checkpoint_draft_mapping(
        &mut self,
        repository: &Repository,
        id: &str,
        identity: &DraftIdentity,
    ) -> Result<(), OutboxError> {
        self.stage_issue_create_state(
            id,
            IssueCreateState::Mapped {
                issue_id: identity.issue_id,
                issue_node_id: identity.issue_node_id.clone(),
                issue_number: identity.issue_number,
                issue_url: identity.issue_url.clone(),
            },
        )?;
        for operation in &mut self.outbox.operations {
            if let MutationPayload::DependencyUpdate { edge, .. } = &mut operation.payload {
                edge.resolve_temporary_id(identity.temporary_id, identity.issue_number);
            }
        }
        self.publish(repository)
    }

    fn stage_issue_create_state(
        &mut self,
        id: &str,
        state: IssueCreateState,
    ) -> Result<(), OutboxError> {
        match &mut self.operation_mut(id)?.payload {
            MutationPayload::IssueCreate { state: current, .. } => {
                *current = state;
                Ok(())
            }
            MutationPayload::PriorityUpdate { .. } | MutationPayload::DependencyUpdate { .. } => {
                Err(OutboxError::InvalidMutationKind(id.to_owned()))
            }
        }
    }

    pub(crate) fn checkpoint_priority_state(
        &mut self,
        repository: &Repository,
        id: &str,
        state: PriorityMutationState,
    ) -> Result<(), OutboxError> {
        self.stage_priority_state(id, state)?;
        self.publish(repository)
    }

    pub(crate) fn stage_dependency_state(
        &mut self,
        id: &str,
        state: DependencyMutationState,
    ) -> Result<(), OutboxError> {
        match &mut self.operation_mut(id)?.payload {
            MutationPayload::DependencyUpdate { state: current, .. } => {
                *current = state;
                Ok(())
            }
            MutationPayload::IssueCreate { .. } | MutationPayload::PriorityUpdate { .. } => {
                Err(OutboxError::InvalidMutationKind(id.to_owned()))
            }
        }
    }

    pub(crate) fn checkpoint_dependency_state(
        &mut self,
        repository: &Repository,
        id: &str,
        state: DependencyMutationState,
    ) -> Result<(), OutboxError> {
        self.stage_dependency_state(id, state)?;
        self.publish(repository)
    }

    pub(crate) fn resolve_remote(
        &mut self,
        repository: &Repository,
        id: &str,
        remote: LogicalPriority,
    ) -> Result<(), OutboxError> {
        self.stage_priority_state(id, PriorityMutationState::ResolvedRemote { remote })?;
        self.publish(repository)
    }

    pub(crate) fn rebase_priority(
        &mut self,
        repository: &Repository,
        id: &str,
        base: LogicalPriority,
        desired: Option<LogicalPriority>,
    ) -> Result<(), OutboxError> {
        self.operation_mut(id)?
            .rebase_priority(base, desired)
            .map_err(|()| OutboxError::InvalidMutationKind(id.to_owned()))?;
        self.publish(repository)
    }

    fn publish(&self, repository: &Repository) -> Result<(), OutboxError> {
        self.outbox.validate(repository)?;
        self.store.publish(&self.outbox)
    }

    pub(crate) fn finalize(
        &mut self,
        repository: &Repository,
        retired: &BTreeSet<String>,
        state_updates: BTreeMap<String, MutationStateUpdate>,
    ) -> Result<(), OutboxError> {
        for (operation_id, state) in state_updates {
            match state {
                MutationStateUpdate::Priority(state) => {
                    self.stage_priority_state(&operation_id, state)?;
                }
                MutationStateUpdate::Dependency(state) => {
                    self.stage_dependency_state(&operation_id, state)?;
                }
            }
        }
        self.outbox
            .operations
            .retain(|operation| !retired.contains(operation.id()));
        for operation in &mut self.outbox.operations {
            operation
                .header
                .depends_on
                .retain(|dependency| !retired.contains(dependency));
        }
        self.operation_index = operation_index(&self.outbox);
        self.publish(repository)
    }

    fn operation_mut(&mut self, id: &str) -> Result<&mut PendingMutation, OutboxError> {
        let index = self
            .operation_index
            .get(id)
            .copied()
            .ok_or_else(|| OutboxError::UnknownOperation(id.to_owned()))?;
        Ok(&mut self.outbox.operations[index])
    }
}

fn operation_index(outbox: &PendingMutationOutbox) -> BTreeMap<String, usize> {
    outbox
        .operations
        .iter()
        .enumerate()
        .map(|(index, operation)| (operation.id().to_owned(), index))
        .collect()
}

fn new_operation_id(repository: &Repository, issue_number: u64) -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = OPERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let material = format!(
        "{}\0{issue_number}\0{elapsed}\0{}\0{sequence}",
        repository.full_name(),
        std::process::id()
    );
    let digest = hex::encode(Sha256::digest(material.as_bytes()));
    format!("op-{}", &digest[..32])
}

#[derive(Debug, Error)]
pub(crate) enum OutboxError {
    #[error("could not determine the Pending mutation state directory: {0}")]
    StateDirectory(StoreError),
    #[error("could not create the Pending mutation directory: {0}")]
    CreateDirectory(io::Error),
    #[error("could not open the Pending mutation lock: {0}")]
    OpenLock(io::Error),
    #[error("could not lock Pending mutations for update: {0}")]
    Lock(io::Error),
    #[error("could not encode Pending mutations: {0}")]
    Encode(serde_json::Error),
    #[error("could not atomically publish Pending mutations: {0}")]
    Publish(AtomicFileError),
    #[error("could not read Pending mutations: {0}")]
    Read(io::Error),
    #[error("could not decode Pending mutations: {0}")]
    Decode(serde_json::Error),
    #[error("unsupported Pending mutation schema {0:?}")]
    UnsupportedSchema(String),
    #[error("Pending mutations belong to {actual}, not {expected}")]
    RepositoryMismatch { expected: String, actual: String },
    #[error("a Pending mutation has an invalid Issue number")]
    InvalidIssueNumber,
    #[error("Pending mutation operation ID {0:?} is duplicated")]
    DuplicateOperationId(String),
    #[error("Pending mutation operation {0:?} has a missing, forward, or self dependency")]
    InvalidDependency(String),
    #[error("a Pending Priority update cannot desire a Priority conflict")]
    InvalidDesiredPriority,
    #[error("Pending mutation operation {0:?} contains an unsafe Priority write plan")]
    InvalidWritePlan(String),
    #[error("Pending mutation operation {0:?} has an invalid Dependency edge")]
    InvalidDependencyEdge(String),
    #[error("Pending mutation operation {0:?} contains an invalid Draft Issue")]
    InvalidDraftIssue(String),
    #[error("Pending mutation operation {0:?} does not exist")]
    UnknownOperation(String),
    #[error("Pending mutation operation {0:?} is not a Priority update")]
    InvalidMutationKind(String),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{OutboxError, PendingMutationOutbox};
    use crate::repository::Repository;

    #[test]
    fn applying_plan_must_exactly_match_the_canonical_safe_plan() {
        let repository = Repository::parse("acme/reconcile").expect("repository");
        let outbox: PendingMutationOutbox = serde_json::from_value(json!({
            "schema_version": "grit.pending-mutations/v1",
            "repository": "acme/reconcile",
            "operations": [{
                "kind": "priority_update",
                "id": "op-unsafe",
                "issue_number": 1,
                "base": {"state": "declared", "value": "p1"},
                "desired": {"state": "declared", "value": "p0"},
                "state": {
                    "status": "applying",
                    "expected_labels": ["priority:p1"],
                    "remaining_writes": [
                        {"action": "add", "label": "priority:p4"},
                        {"action": "remove", "label": "priority:p4"},
                        {"action": "add", "label": "priority:p0"},
                        {"action": "remove", "label": "priority:p1"}
                    ]
                }
            }]
        }))
        .expect("syntactically valid outbox");

        assert!(matches!(
            outbox.validate(&repository),
            Err(OutboxError::InvalidWritePlan(operation)) if operation == "op-unsafe"
        ));
    }
}
