use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use thiserror::Error;

pub(crate) fn publish(path: &Path, prefix: &str, bytes: &[u8]) -> Result<(), AtomicFileError> {
    let parent = path.parent().ok_or(AtomicFileError::MissingParent)?;
    fs::create_dir_all(parent).map_err(AtomicFileError::CreateDirectory)?;
    let temporary = create_temporary(parent, prefix)?;
    let temporary_path = temporary.path.clone();
    let result = publish_temporary(temporary, &temporary_path, path, parent, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(AtomicFileError::Publish)
}

struct TemporaryFile {
    path: PathBuf,
    file: File,
}

fn create_temporary(directory: &Path, prefix: &str) -> Result<TemporaryFile, AtomicFileError> {
    for attempt in 0..1000_u32 {
        let path = directory.join(format!(".{prefix}-{}-{attempt}.tmp", std::process::id()));
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
            Err(error) => return Err(AtomicFileError::CreateTemporary(error)),
        }
    }
    Err(AtomicFileError::TemporaryNameExhausted)
}

fn publish_temporary(
    mut temporary: TemporaryFile,
    temporary_path: &Path,
    destination: &Path,
    parent: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    temporary.file.write_all(bytes)?;
    temporary.file.sync_all()?;
    drop(temporary.file);
    fs::rename(temporary_path, destination)?;
    sync_directory(parent)
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
pub(crate) enum AtomicFileError {
    #[error("the destination has no parent directory")]
    MissingParent,
    #[error("could not create the destination directory: {0}")]
    CreateDirectory(io::Error),
    #[error("could not create an atomic candidate: {0}")]
    CreateTemporary(io::Error),
    #[error("could not find a unique atomic candidate name")]
    TemporaryNameExhausted,
    #[error("could not publish the atomic candidate: {0}")]
    Publish(io::Error),
}
