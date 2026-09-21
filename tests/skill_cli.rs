use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn hyfa(project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command
        .current_dir(project)
        .env("HYFA_NO_KEYRING", "1")
        .env_remove("GH_TOKEN")
        .env("HYFA_GITHUB_API_URL", "invalid-api-must-not-be-read")
        .env("PATH", "");
    command
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn default_install_copies_the_embedded_skill_offline_into_the_current_project() {
    let project = tempfile::tempdir().unwrap();
    let output = success(
        hyfa(project.path())
            .args(["skill", "install"])
            .output()
            .unwrap(),
    );
    let destination = project.path().join(".agents/skills/hyfa");
    assert!(
        !fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(destination.join("SKILL.md")).unwrap(),
        include_str!("../.skill/SKILL.md")
    );
    assert!(output.contains(destination.canonicalize().unwrap().to_str().unwrap()));
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[test]
fn provider_normalization_avoids_duplicates_and_explicit_project_root_is_honored() {
    let working = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let output = success(
        hyfa(working.path())
            .args([
                "skill",
                "install",
                "--providers",
                "codex,universal,claude-code,codex",
                "--project-root",
            ])
            .arg(project.path())
            .output()
            .unwrap(),
    );
    assert!(
        project
            .path()
            .join(".agents/skills/hyfa/SKILL.md")
            .is_file()
    );
    assert!(
        project
            .path()
            .join(".claude/skills/hyfa/SKILL.md")
            .is_file()
    );
    assert!(!project.path().join(".codex").exists());
    assert!(fs::read_dir(working.path()).unwrap().next().is_none());
    assert!(output.contains("normalized"));
}

#[test]
fn existing_install_is_preserved_until_force_is_explicit() {
    let project = tempfile::tempdir().unwrap();
    success(
        hyfa(project.path())
            .args(["skill", "install"])
            .output()
            .unwrap(),
    );
    let destination = project.path().join(".agents/skills/hyfa");
    fs::write(destination.join("SKILL.md"), "local customization").unwrap();
    fs::write(destination.join("local.txt"), "local file").unwrap();
    let output = hyfa(project.path())
        .args(["skill", "install"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--force"));
    assert_eq!(
        fs::read_to_string(destination.join("SKILL.md")).unwrap(),
        "local customization"
    );
    success(
        hyfa(project.path())
            .args(["skill", "install", "--force"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(destination.join("SKILL.md")).unwrap(),
        include_str!("../.skill/SKILL.md")
    );
    assert!(!destination.join("local.txt").exists());
}

#[test]
fn all_existing_destinations_are_checked_before_any_provider_is_installed() {
    let project = tempfile::tempdir().unwrap();
    let existing = project.path().join(".claude/skills/hyfa");
    fs::create_dir_all(&existing).unwrap();
    fs::write(existing.join("SKILL.md"), "keep this").unwrap();
    let output = hyfa(project.path())
        .args(["skill", "install", "--providers", "universal,claude-code"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!project.path().join(".agents").exists());
    assert_eq!(
        fs::read_to_string(existing.join("SKILL.md")).unwrap(),
        "keep this"
    );
}

#[test]
fn invalid_providers_and_user_scope_misconfiguration_do_not_write_files() {
    let project = tempfile::tempdir().unwrap();
    for args in [
        vec!["skill", "install", "--providers", "unknown-provider"],
        vec!["skill", "install", "--scope", "user", "--project-root", "."],
        vec![
            "skill",
            "install",
            "--scope",
            "user",
            "--providers",
            "codex",
        ],
    ] {
        let output = hyfa(project.path()).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(fs::read_dir(project.path()).unwrap().next().is_none());
    }
}

#[test]
fn providers_and_help_are_available_without_authentication() {
    let project = tempfile::tempdir().unwrap();
    let output = success(
        hyfa(project.path())
            .args(["skill", "providers"])
            .output()
            .unwrap(),
    );
    assert!(output.lines().any(|line| line.starts_with("codex\t")));
    assert!(output.lines().any(|line| line.starts_with("universal\t")));
    let output = success(
        hyfa(project.path())
            .args(["skill", "install", "--help"])
            .output()
            .unwrap(),
    );
    for flag in ["--providers", "--scope", "--project-root", "--force"] {
        assert!(output.contains(flag));
    }
}

#[cfg(unix)]
#[test]
fn symlinked_destinations_and_ancestors_are_never_followed_even_with_force() {
    use std::os::unix::fs::symlink;
    for ancestor in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep.txt"), "outside").unwrap();
        if ancestor {
            symlink(outside.path(), project.path().join(".agents")).unwrap();
        } else {
            fs::create_dir_all(project.path().join(".agents/skills")).unwrap();
            symlink(outside.path(), project.path().join(".agents/skills/hyfa")).unwrap();
        }
        let output = hyfa(project.path())
            .args(["skill", "install", "--force"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
        assert_eq!(
            fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
            "outside"
        );
    }
}

#[cfg(unix)]
#[test]
fn broken_destination_symlink_is_preserved() {
    let project = tempfile::tempdir().unwrap();
    let parent = project.path().join(".agents/skills");
    fs::create_dir_all(&parent).unwrap();
    let link = parent.join("hyfa");
    std::os::unix::fs::symlink(project.path().join("missing"), &link).unwrap();
    let output = hyfa(project.path())
        .args(["skill", "install"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}
