use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    atomic_file::{self, AtomicFileError},
    github::CreatedIssueIdentity,
    model::TemporaryIssueId,
    repository::Repository,
    store::{StoreError, repository_state_directory},
};

const SCHEMA_VERSION: &str = "grit.draft-identities/v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DraftIdentity {
    pub(crate) temporary_id: TemporaryIssueId,
    pub(crate) synthetic_number: u64,
    pub(crate) issue_id: u64,
    pub(crate) issue_node_id: String,
    pub(crate) issue_number: u64,
    pub(crate) issue_url: String,
}

#[derive(Deserialize, Serialize)]
struct IdentityDocument {
    schema_version: String,
    repository: String,
    identities: BTreeMap<TemporaryIssueId, DraftIdentity>,
}

pub(crate) struct DraftIdentityStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl DraftIdentityStore {
    pub(crate) fn discover(repository: &Repository) -> Result<Self, DraftIdentityError> {
        let directory =
            repository_state_directory(repository).map_err(DraftIdentityError::StateDirectory)?;
        Ok(Self {
            path: directory.join("draft-identities.json"),
            lock_path: directory.join("draft-identities.lock"),
        })
    }

    pub(crate) fn begin_transaction(
        &self,
        repository: &Repository,
    ) -> Result<DraftIdentityTransaction<'_>, DraftIdentityError> {
        let parent = self.path.parent().expect("identity path has a parent");
        fs::create_dir_all(parent).map_err(DraftIdentityError::CreateDirectory)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&self.lock_path)
            .map_err(DraftIdentityError::OpenLock)?;
        FileExt::lock_exclusive(&lock).map_err(DraftIdentityError::Lock)?;
        let document = self.load(repository)?;
        let remote_identities = RemoteIdentityIndex::from_document(&document)?;
        Ok(DraftIdentityTransaction {
            store: self,
            _lock: lock,
            document,
            remote_identities,
        })
    }

    fn load(&self, repository: &Repository) -> Result<IdentityDocument, DraftIdentityError> {
        let document: IdentityDocument = match File::open(&self.path) {
            Ok(file) => serde_json::from_reader(file).map_err(DraftIdentityError::Decode)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => IdentityDocument {
                schema_version: SCHEMA_VERSION.to_owned(),
                repository: repository.full_name().to_owned(),
                identities: BTreeMap::new(),
            },
            Err(error) => return Err(DraftIdentityError::Read(error)),
        };
        if document.schema_version != SCHEMA_VERSION {
            return Err(DraftIdentityError::UnsupportedSchema(
                document.schema_version,
            ));
        }
        if !document
            .repository
            .eq_ignore_ascii_case(repository.full_name())
        {
            return Err(DraftIdentityError::RepositoryMismatch);
        }
        Ok(document)
    }

    fn publish(&self, document: &IdentityDocument) -> Result<(), DraftIdentityError> {
        let mut bytes = serde_json::to_vec_pretty(document).map_err(DraftIdentityError::Encode)?;
        bytes.push(b'\n');
        atomic_file::publish(&self.path, "draft-identities", &bytes)
            .map_err(DraftIdentityError::Publish)
    }
}

pub(crate) struct DraftIdentityTransaction<'store> {
    store: &'store DraftIdentityStore,
    _lock: File,
    document: IdentityDocument,
    remote_identities: RemoteIdentityIndex,
}

#[derive(Default)]
struct RemoteIdentityIndex {
    issue_ids: BTreeMap<u64, TemporaryIssueId>,
    node_ids: BTreeMap<String, TemporaryIssueId>,
    issue_numbers: BTreeMap<u64, TemporaryIssueId>,
}

impl RemoteIdentityIndex {
    fn from_document(document: &IdentityDocument) -> Result<Self, DraftIdentityError> {
        let mut index = Self::default();
        for identity in document.identities.values() {
            index.claim(identity)?;
        }
        Ok(index)
    }

    fn claim(&mut self, identity: &DraftIdentity) -> Result<(), DraftIdentityError> {
        let conflicting = self
            .issue_ids
            .get(&identity.issue_id)
            .or_else(|| self.node_ids.get(&identity.issue_node_id))
            .or_else(|| self.issue_numbers.get(&identity.issue_number))
            .copied()
            .filter(|existing| *existing != identity.temporary_id);
        if let Some(existing) = conflicting {
            return Err(DraftIdentityError::RemoteIdentityConflict {
                issue_number: identity.issue_number,
                existing,
                attempted: identity.temporary_id,
            });
        }
        self.issue_ids
            .insert(identity.issue_id, identity.temporary_id);
        self.node_ids
            .insert(identity.issue_node_id.clone(), identity.temporary_id);
        self.issue_numbers
            .insert(identity.issue_number, identity.temporary_id);
        Ok(())
    }
}

impl DraftIdentityTransaction<'_> {
    pub(crate) fn resolve(&self, temporary_id: TemporaryIssueId) -> Option<DraftIdentity> {
        self.document.identities.get(&temporary_id).cloned()
    }

    pub(crate) fn record(
        &mut self,
        temporary_id: TemporaryIssueId,
        synthetic_number: u64,
        remote: &CreatedIssueIdentity,
    ) -> Result<DraftIdentity, DraftIdentityError> {
        let identity = DraftIdentity {
            temporary_id,
            synthetic_number,
            issue_id: remote.id,
            issue_node_id: remote.node_id.clone(),
            issue_number: remote.number,
            issue_url: remote.url.clone(),
        };
        if let Some(existing) = self.document.identities.get(&temporary_id) {
            if existing.synthetic_number != identity.synthetic_number
                || existing.issue_id != identity.issue_id
                || existing.issue_node_id != identity.issue_node_id
                || existing.issue_number != identity.issue_number
                || existing.issue_url != identity.issue_url
            {
                return Err(DraftIdentityError::MappingConflict(temporary_id));
            }
            return Ok(existing.clone());
        }
        self.remote_identities.claim(&identity)?;
        self.document
            .identities
            .insert(temporary_id, identity.clone());
        self.store.publish(&self.document)?;
        Ok(identity)
    }
}

#[derive(Debug, Error)]
pub(crate) enum DraftIdentityError {
    #[error("could not determine the Draft identity state directory: {0}")]
    StateDirectory(StoreError),
    #[error("could not read Draft identities: {0}")]
    Read(io::Error),
    #[error("could not create the Draft identity state directory: {0}")]
    CreateDirectory(io::Error),
    #[error("could not open the Draft identity lock: {0}")]
    OpenLock(io::Error),
    #[error("could not lock Draft identities: {0}")]
    Lock(io::Error),
    #[error("could not decode Draft identities: {0}")]
    Decode(serde_json::Error),
    #[error("could not encode Draft identities: {0}")]
    Encode(serde_json::Error),
    #[error("could not atomically publish Draft identities: {0}")]
    Publish(AtomicFileError),
    #[error("unsupported Draft identity schema {0:?}")]
    UnsupportedSchema(String),
    #[error("Draft identities belong to another Repository")]
    RepositoryMismatch,
    #[error("Temporary Issue ID {0} is already mapped to another GitHub Issue")]
    MappingConflict(TemporaryIssueId),
    #[error(
        "GitHub Issue #{issue_number} is already mapped to Temporary Issue ID {existing}, not {attempted}"
    )]
    RemoteIdentityConflict {
        issue_number: u64,
        existing: TemporaryIssueId,
        attempted: TemporaryIssueId,
    },
}

impl DraftIdentityError {
    pub(crate) fn is_mapping_conflict(&self) -> bool {
        matches!(
            self,
            Self::MappingConflict(_) | Self::RemoteIdentityConflict { .. }
        )
    }
}
