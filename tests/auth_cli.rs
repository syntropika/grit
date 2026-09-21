use std::process::Command;

fn grit() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command
        .env("GRIT_NO_KEYRING", "1")
        .env_remove("GH_TOKEN")
        .env_remove("GRIT_GITHUB_CLIENT_ID")
        .env_remove("GRIT_GITHUB_HOST")
        .env("PATH", "");
    command
}

#[test]
fn auth_help_exposes_native_login_without_a_token_argument() {
    let output = grit().args(["auth", "login", "--help"]).output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--client-id"));
    assert!(help.contains("Defaults to Grit's app on github.com"));
    assert!(help.contains("other hosts require their own app"));
    assert!(help.contains("--with-token"));
    assert!(!help.contains("--token <"));
}

#[test]
fn github_browser_login_uses_the_default_app_before_checking_secure_storage() {
    let output = grit().args(["auth", "login"]).output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("secure credential store"));
    assert!(!error.contains("GRIT_GITHUB_CLIENT_ID"));
}

#[test]
fn enterprise_browser_login_requires_its_own_app_before_touching_store_or_network() {
    let output = grit()
        .args(["auth", "login", "--hostname", "github.example.com"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("this hostname requires its own"));
    assert!(error.contains("GRIT_GITHUB_CLIENT_ID"));
    assert!(error.contains("--with-token"));
}

#[test]
fn invalid_client_id_overrides_fail_instead_of_using_the_official_app() {
    for invalid in ["", "invalid/app"] {
        let output = grit()
            .env("GRIT_GITHUB_CLIENT_ID", invalid)
            .args(["auth", "login"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("client ID is invalid"));

        let output = grit()
            .env("GRIT_GITHUB_CLIENT_ID", "environment-app")
            .args(["auth", "login", "--client-id", invalid])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("client ID is invalid"));
    }
}

#[test]
fn explicit_client_id_wins_over_invalid_environment_before_checking_secure_storage() {
    let output = grit()
        .env("GRIT_GITHUB_CLIENT_ID", "invalid/app")
        .args([
            "auth",
            "login",
            "--hostname",
            "github.example.com",
            "--client-id",
            "flag-app",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("secure credential store"));
}

#[test]
fn explicit_keyring_opt_out_prevents_login_and_logout_from_touching_operator_credentials() {
    for args in [
        vec!["auth", "login", "--with-token"],
        vec!["auth", "logout"],
    ] {
        let output = grit().args(args).output().unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("secure credential store"));
        assert!(error.contains("GH_TOKEN"));
    }
}

#[test]
fn invalid_host_is_rejected_before_network_or_keychain_access() {
    let output = grit()
        .args([
            "auth",
            "status",
            "--hostname",
            "https://user:secret@example.com/path",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("DNS name"));
    assert!(!error.contains("secret"));
    assert!(output.stdout.is_empty());
}

#[test]
fn unauthenticated_status_is_unsuccessful_and_does_not_claim_an_account() {
    let output = grit().args(["auth", "status", "--json"]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("grit auth login"));
}
