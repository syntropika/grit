use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use thiserror::Error;

use crate::{
    atomic_file::{self, AtomicFileError},
    model::{LocalReplica, ReplicaError},
    repository::Repository,
};

pub(crate) struct ReplicaStore {
    replica_path: PathBuf,
}

impl ReplicaStore {
    pub(crate) fn discover(repository: &Repository) -> Result<Self, StoreError> {
        let replica_path = repository_state_directory(repository)?.join("replica.json");
        Ok(Self { replica_path })
    }

    pub(crate) fn publish(&self, replica: &LocalReplica) -> Result<(), StoreError> {
        let mut bytes = serde_json::to_vec_pretty(replica).map_err(StoreError::Encode)?;
        bytes.push(b'\n');
        atomic_file::publish(&self.replica_path, "replica", &bytes).map_err(StoreError::Publish)
    }

    pub(crate) fn load(&self, repository: &Repository) -> Result<LocalReplica, StoreError> {
        let file = File::open(&self.replica_path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => StoreError::MissingReplica,
            _ => StoreError::Read(error),
        })?;
        let replica: LocalReplica = serde_json::from_reader(file).map_err(StoreError::Decode)?;
        replica
            .validate(repository.full_name())
            .map_err(StoreError::InvalidReplica)?;
        Ok(replica)
    }

    pub(crate) fn repository_directory(&self) -> &Path {
        self.replica_path
            .parent()
            .expect("replica path always has a parent")
    }
}

pub(crate) fn repository_state_directory(repository: &Repository) -> Result<PathBuf, StoreError> {
    let root = match std::env::var_os("GRIT_STATE_DIR") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => ProjectDirs::from("", "", "grit")
            .ok_or(StoreError::NoStateDirectory)?
            .data_local_dir()
            .to_owned(),
    };
    Ok(root
        .join("repositories")
        .join(repository.owner().to_ascii_lowercase())
        .join(repository.name().to_ascii_lowercase()))
}

#[derive(Debug, Error)]
pub(crate) enum StoreError {
    #[error("could not determine a local state directory")]
    NoStateDirectory,
    #[error("could not encode the normalized Local replica: {0}")]
    Encode(serde_json::Error),
    #[error("could not atomically publish the Local replica: {0}")]
    Publish(AtomicFileError),
    #[error("no Local replica exists for this Repository")]
    MissingReplica,
    #[error("could not read the Local replica: {0}")]
    Read(io::Error),
    #[error("could not decode the Local replica: {0}")]
    Decode(serde_json::Error),
    #[error("Local replica is invalid: {0}")]
    InvalidReplica(ReplicaError),
}
