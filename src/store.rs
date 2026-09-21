use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use thiserror::Error;

use crate::{model::LocalReplica, repository::Repository};

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

        let mut temporary = tempfile_in(parent)?;
        let temporary_path = temporary.path.clone();
        let result = (|| {
            temporary.file.write_all(&bytes)?;
            temporary.file.sync_all()?;
            drop(temporary.file);
            fs::rename(&temporary_path, &self.replica_path)?;
            sync_directory(parent)?;
            Ok::<(), io::Error>(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result.map_err(StoreError::Publish)
    }
}

struct TemporaryFile {
    path: PathBuf,
    file: File,
}

fn tempfile_in(directory: &Path) -> Result<TemporaryFile, StoreError> {
    for attempt in 0..1000_u32 {
        let path = directory.join(format!(".replica-{}-{attempt}.tmp", std::process::id()));
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
            Err(error) => return Err(StoreError::CreateTemporary(error)),
        }
    }
    Err(StoreError::TemporaryNameExhausted)
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
}
