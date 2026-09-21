use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use thiserror::Error;

use crate::{
    model::{LocalReplica, ReplicaError},
    repository::Repository,
};

pub(crate) struct ReplicaStore {
    replica_path: PathBuf,
}

impl ReplicaStore {
    pub(crate) fn discover(repository: &Repository) -> Result<Self, StoreError> {
        let root = match std::env::var_os("GRIT_STATE_DIR") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => ProjectDirs::from("", "", "grit")
                .ok_or(StoreError::NoStateDirectory)?
                .data_local_dir()
                .to_owned(),
        };
        let replica_path = root
            .join("repositories")
            .join(repository.owner().to_ascii_lowercase())
            .join(repository.name().to_ascii_lowercase())
            .join("replica.json");
        Ok(Self { replica_path })
    }

    pub(crate) fn publish(&self, replica: &LocalReplica) -> Result<(), StoreError> {
        let parent = self
            .replica_path
            .parent()
            .expect("replica path always has a parent");
        fs::create_dir_all(parent).map_err(StoreError::CreateDirectory)?;

        let mut bytes = serde_json::to_vec_pretty(replica).map_err(StoreError::Encode)?;
        bytes.push(b'\n');

        publish_bytes_atomically(&self.replica_path, &bytes, ".replica").map_err(
            |error| match error {
                AtomicWriteError::CreateTemporary(error) => StoreError::CreateTemporary(error),
                AtomicWriteError::TemporaryNameExhausted => StoreError::TemporaryNameExhausted,
                AtomicWriteError::Publish(error) => StoreError::Publish(error),
            },
        )
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

struct TemporaryFile {
    path: PathBuf,
    file: File,
}

fn tempfile_in(directory: &Path, prefix: &str) -> Result<TemporaryFile, AtomicWriteError> {
    for attempt in 0..1000_u32 {
        let path = directory.join(format!("{prefix}-{}-{attempt}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok(TemporaryFile { path, file }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(AtomicWriteError::CreateTemporary(error)),
        }
    }
    Err(AtomicWriteError::TemporaryNameExhausted)
}

pub(crate) fn publish_bytes_atomically(
    destination: &Path,
    bytes: &[u8],
    temporary_prefix: &str,
) -> Result<(), AtomicWriteError> {
    let parent = destination
        .parent()
        .expect("atomic publication destination always has a parent");
    let mut temporary = tempfile_in(parent, temporary_prefix)?;
    let temporary_path = temporary.path.clone();
    let result = (|| {
        temporary.file.write_all(bytes)?;
        temporary.file.sync_all()?;
        drop(temporary.file);
        fs::rename(&temporary_path, destination)?;
        sync_directory(parent)?;
        Ok::<(), io::Error>(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(AtomicWriteError::Publish)
}

pub(crate) enum AtomicWriteError {
    CreateTemporary(io::Error),
    TemporaryNameExhausted,
    Publish(io::Error),
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[derive(Debug, Error)]
pub(crate) enum StoreError {
    #[error("could not determine a local state directory")]
    NoStateDirectory,
    #[error("could not create the Local replica directory: {0}")]
    CreateDirectory(io::Error),
    #[error("could not create an atomic Local replica candidate: {0}")]
    CreateTemporary(io::Error),
    #[error("could not find a unique atomic Local replica candidate name")]
    TemporaryNameExhausted,
    #[error("could not encode the normalized Local replica: {0}")]
    Encode(serde_json::Error),
    #[error("could not atomically publish the Local replica: {0}")]
    Publish(io::Error),
    #[error("no Local replica exists for this Repository")]
    MissingReplica,
    #[error("could not read the Local replica: {0}")]
    Read(io::Error),
    #[error("could not decode the Local replica: {0}")]
    Decode(serde_json::Error),
    #[error("Local replica is invalid: {0}")]
    InvalidReplica(ReplicaError),
}
