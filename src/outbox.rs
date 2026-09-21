use std::{
    collections::BTreeSet,
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
        for operation in &self.operations {
            if operation.issue_number() == 0 {
                return Err(OutboxError::InvalidIssueNumber);
            }
            if !operation_ids.insert(operation.id()) {
                return Err(OutboxError::DuplicateOperationId(operation.id().to_owned()));
            }
            if matches!(operation.desired(), LogicalPriority::Conflict { .. }) {
                return Err(OutboxError::InvalidDesiredPriority);
            }
        }
        Ok(())
    }

    pub(crate) fn operations(&self) -> &[PendingMutation] {
        &self.operations
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PendingMutation {
    PriorityUpdate {
        id: String,
        issue_number: u64,
        base: LogicalPriority,
        desired: LogicalPriority,
    },
}

impl PendingMutation {
    pub(crate) fn priority_update(
        repository: &Repository,
        issue_number: u64,
        base: LogicalPriority,
        desired: LogicalPriority,
    ) -> Self {
        Self::PriorityUpdate {
            id: new_operation_id(repository, issue_number),
            issue_number,
            base,
            desired,
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::PriorityUpdate { id, .. } => id,
        }
    }

    pub(crate) fn issue_number(&self) -> u64 {
        match self {
            Self::PriorityUpdate { issue_number, .. } => *issue_number,
        }
    }

    pub(crate) fn base(&self) -> &LogicalPriority {
        match self {
            Self::PriorityUpdate { base, .. } => base,
        }
    }

    pub(crate) fn desired(&self) -> &LogicalPriority {
        match self {
            Self::PriorityUpdate { desired, .. } => desired,
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
        Ok(OutboxTransaction {
            store: self,
            _lock: lock,
            outbox,
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
    #[error("a Pending Priority update cannot desire a Priority conflict")]
    InvalidDesiredPriority,
}
