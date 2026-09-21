use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use clap::Subcommand;
use skillinstaller::{
    EmbeddedSkill, InstallMethod, InstallRequest, InstallerError, ProviderId, Scope, SkillSource,
    install, is_agents_provider, normalize_providers, parse_providers_csv, parse_skill,
    print_install_result, resolve_install_target, supported_providers,
};
use thiserror::Error;

const SKILL_MARKDOWN: &str = include_str!("../.skill/SKILL.md");

#[derive(Subcommand)]
pub(crate) enum SkillCommand {
    /// Copy the bundled skill into the selected providers' skill directories, without network access.
    Install {
        /// Comma-separated provider names, or '*' for all; see hyfa skill providers.
        #[arg(long, default_value = "universal")]
        providers: String,
        /// User scope is supported only for providers with a dedicated user skill directory.
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
        /// Existing project directory; defaults to the current directory for project scope.
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Replace existing skill directories. Symlinks are never overwritten.
        #[arg(long)]
        force: bool,
    },
    /// List supported provider names and their project skill directories.
    Providers,
}

pub(crate) fn execute(command: SkillCommand) -> Result<(), SkillError> {
    match command {
        SkillCommand::Providers => {
            for provider in supported_providers() {
                println!("{}\t{}", provider.id.as_str(), provider.project_path);
            }
        }
        SkillCommand::Install {
            providers,
            scope,
            project_root,
            force,
        } => {
            let request = request(&providers, scope, project_root, force, env::current_dir)?;
            preflight(&request, |provider| {
                Ok(resolve_install_target(
                    provider,
                    request.scope,
                    request.project_root.as_deref(),
                )?
                .target_dir)
            })?;
            print_install_result(&install(request)?);
        }
    }
    Ok(())
}

fn request(
    providers: &str,
    scope: Scope,
    project_root: Option<PathBuf>,
    force: bool,
    current_dir: impl FnOnce() -> io::Result<PathBuf>,
) -> Result<InstallRequest, SkillError> {
    let providers = parse_providers_csv(providers)?;
    let project_root = match scope {
        Scope::Project => {
            let root = project_root.map(Ok).unwrap_or_else(current_dir)?;
            let root = root.canonicalize()?;
            if !root.is_dir() {
                return Err(SkillError::NotDirectory(root));
            }
            Some(root)
        }
        Scope::User if project_root.is_some() => return Err(SkillError::UserProjectRoot),
        Scope::User => {
            if let Some(provider) = providers
                .iter()
                .find(|provider| is_agents_provider(**provider))
            {
                return Err(SkillError::UnsupportedUserProvider(provider.as_str()));
            }
            None
        }
    };
    Ok(InstallRequest {
        source: SkillSource::Embedded(EmbeddedSkill {
            skill_md: SKILL_MARKDOWN.to_owned(),
            files: Vec::new(),
        }),
        providers,
        scope,
        project_root,
        method: InstallMethod::Copy,
        force,
    })
}

fn preflight(
    request: &InstallRequest,
    mut resolve: impl FnMut(ProviderId) -> Result<PathBuf, SkillError>,
) -> Result<(), SkillError> {
    let skill = parse_skill(&request.source)?;
    let (providers, _) = normalize_providers(&request.providers);
    // Check every destination before the library starts copying any provider.
    for provider in providers {
        let parent = resolve(provider)?;
        if !parent.is_absolute() {
            return Err(SkillError::RelativeTarget(parent));
        }
        for ancestor in parent.ancestors() {
            if let Some(metadata) = metadata(ancestor)? {
                if metadata.file_type().is_symlink() {
                    return Err(SkillError::Symlink(ancestor.to_owned()));
                }
                if !metadata.is_dir() {
                    return Err(SkillError::NotDirectory(ancestor.to_owned()));
                }
            }
        }
        let destination = parent.join(&skill.name);
        if let Some(metadata) = metadata(&destination)? {
            if metadata.file_type().is_symlink() {
                return Err(SkillError::Symlink(destination));
            }
            if !request.force {
                return Err(InstallerError::AlreadyExists { path: destination }.into());
            }
            if !metadata.is_dir() {
                return Err(SkillError::NotDirectory(destination));
            }
        }
        let staging = parent.join(format!(".{}.tmp-{}", skill.name, std::process::id()));
        if metadata(&staging)?.is_some() {
            return Err(SkillError::ExistingStaging(staging));
        }
    }
    Ok(())
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>, io::Error> {
    match fs::symlink_metadata(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Error)]
pub(crate) enum SkillError {
    #[error(transparent)]
    Installer(#[from] InstallerError),
    #[error("could not install the bundled skill: {0}")]
    Io(#[from] io::Error),
    #[error("--project-root applies only to --scope project")]
    UserProjectRoot,
    #[error(
        "user scope is not supported for provider '{0}' by the bundled installer; use --scope project"
    )]
    UnsupportedUserProvider(&'static str),
    #[error(
        "skill destination must resolve to an absolute path; check provider configuration: {0}"
    )]
    RelativeTarget(PathBuf),
    #[error("expected a directory at {0}")]
    NotDirectory(PathBuf),
    #[error("refusing to install through or replace a symlink: {0}")]
    Symlink(PathBuf),
    #[error("skill staging path already exists; inspect it before retrying: {0}")]
    ExistingStaging(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_user_scope_uses_injected_targets_without_project_or_home_discovery() {
        let target = tempfile::tempdir().unwrap();
        let request = request("claude-code", Scope::User, None, false, || {
            panic!("user scope must not inspect the current project")
        })
        .unwrap();
        assert_eq!(request.scope, Scope::User);
        assert!(request.project_root.is_none());
        let mut resolved = Vec::new();
        preflight(&request, |provider| {
            resolved.push(provider);
            Ok(target.path().canonicalize().unwrap().join("skills"))
        })
        .unwrap();
        assert_eq!(resolved, [ProviderId::ClaudeCode]);
        assert!(fs::read_dir(target.path()).unwrap().next().is_none());
    }

    #[test]
    fn existing_library_staging_path_is_preserved_before_copying() {
        let project = tempfile::tempdir().unwrap();
        let parent = project.path().canonicalize().unwrap();
        let request = request(
            "universal",
            Scope::Project,
            Some(parent.clone()),
            true,
            || panic!(),
        )
        .unwrap();
        let staging = parent.join(format!(".hyfa.tmp-{}", std::process::id()));
        fs::write(&staging, "existing work").unwrap();
        assert!(matches!(
            preflight(&request, |_| Ok(parent.clone())),
            Err(SkillError::ExistingStaging(_))
        ));
        assert_eq!(fs::read_to_string(staging).unwrap(), "existing work");
    }
}
