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
    assert!(help.contains("--with-token"));
    assert!(!help.contains("--token <"));
}

#[test]
fn browser_login_without_client_id_explains_configuration_before_touching_store_or_network() {
    let output = grit().args(["auth", "login"]).output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("GRIT_GITHUB_CLIENT_ID"));
    assert!(error.contains("--with-token"));
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
