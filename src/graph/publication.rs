use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use super::GraphError;

pub(super) fn publish(output: &Path, files: &[(&str, Vec<u8>)]) -> Result<(), GraphError> {
    publish_validated(output, files, |_| Ok(()))
}

pub(super) fn publish_validated(
    output: &Path,
    files: &[(&str, Vec<u8>)],
    validate: impl FnOnce(&Path) -> Result<(), GraphError>,
) -> Result<(), GraphError> {
    validate_output_path(output)?;
    let parent = usable_parent(output);
    fs::create_dir_all(parent).map_err(GraphError::CreateParent)?;
    let staging = create_staging_directory(parent, output.file_name().expect("validated name"))?;
    let result = (|| {
        for (name, bytes) in files {
            write_synced(&staging.join(name), bytes)?;
        }
        validate(&staging)?;
        sync_directory(&staging).map_err(GraphError::WriteArtifact)?;
        replace_target(parent, output, &staging)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn validate_output_path(output: &Path) -> Result<(), GraphError> {
    if output.file_name().is_none()
        || matches!(output.file_name(), Some(name) if name == OsStr::new(".") || name == OsStr::new(".."))
    {
        return Err(GraphError::UnsafeOutputPath);
    }
    if let Ok(metadata) = fs::symlink_metadata(output)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(GraphError::UnsafeOutputTarget);
    }
    Ok(())
}

fn usable_parent(output: &Path) -> &Path {
    output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_staging_directory(parent: &Path, name: &OsStr) -> Result<PathBuf, GraphError> {
    for attempt in 0..1000_u32 {
        let path = parent.join(format!(
            ".grit-{}-{}-{attempt}.stage",
            name.to_string_lossy(),
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(GraphError::CreateStaging(error)),
        }
    }
    Err(GraphError::StagingNameExhausted)
}

fn replace_target(parent: &Path, output: &Path, staging: &Path) -> Result<(), GraphError> {
    if !output.exists() {
        fs::rename(staging, output).map_err(GraphError::Publish)?;
        sync_directory(parent).map_err(GraphError::Publish)?;
        return Ok(());
    }

    atomic_exchange(staging, output)?;
    sync_directory(parent).map_err(GraphError::Publish)?;
    fs::remove_dir_all(staging).map_err(GraphError::RemovePrevious)?;
    sync_directory(parent).map_err(GraphError::Publish)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn atomic_exchange(left: &Path, right: &Path) -> Result<(), GraphError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, left, CWD, right, RenameFlags::EXCHANGE)
        .map_err(|error| GraphError::Publish(error.into()))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn atomic_exchange(_left: &Path, _right: &Path) -> Result<(), GraphError> {
    Err(GraphError::AtomicReplacementUnavailable)
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), GraphError> {
    let mut file = File::create(path).map_err(GraphError::WriteArtifact)?;
    file.write_all(bytes).map_err(GraphError::WriteArtifact)?;
    file.sync_all().map_err(GraphError::WriteArtifact)
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::replace_target;

    #[test]
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    fn replacement_exchanges_complete_directories_without_removing_the_target() {
        let parent = TempDir::new().expect("temporary parent");
        let output = parent.path().join("site");
        let staging = parent.path().join("stage");
        fs::create_dir(&output).expect("previous site");
        fs::create_dir(&staging).expect("staged site");
        fs::write(output.join("old.txt"), "previous").expect("previous artifact");
        fs::write(staging.join("new.txt"), "next").expect("next artifact");

        replace_target(parent.path(), &output, &staging).expect("atomic replacement");

        assert_eq!(
            fs::read_to_string(output.join("new.txt")).expect("published site"),
            "next"
        );
        assert!(!staging.exists());
    }
}
