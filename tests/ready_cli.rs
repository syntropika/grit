use std::{fs, process::Command};

use mockito::Server;
use serde_json::Value;
use tempfile::TempDir;

mod support;

use support::{
    assert_success, external_blocker, internal_blocker, issue, mock_repository,
    mock_repository_with_calls,
};

#[test]
fn ready_separates_readiness_from_default_and_assignee_execution_scopes() {
    let mut github = Server::new();
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![internal_blocker(1, "open")]),
        (4, vec![internal_blocker(5, "closed")]),
        (5, vec![]),
    ];
    let mocks = mock_repository_with_calls(
        &mut github,
        "acme/widgets",
        issue_inventory(),
        dependencies,
        2,
    );

    let state = TempDir::new().expect("temporary state directory");
    let default = ready_command(&state, &github.url(), None)
        .output()
        .expect("run grit ready");
    assert_success(&default);
    let default: Value = serde_json::from_slice(&default.stdout).expect("ready JSON");
    assert_eq!(default["schema_version"], "grit.ready/v1");
    assert_eq!(default["command"], "ready");
    assert_eq!(default["repository"], "acme/widgets");
    assert_eq!(default["source"], "live");
    assert_eq!(default["execution_scope"]["mode"], "available");
    assert_eq!(issue_numbers(&default), vec![1, 4]);
    assert_eq!(default["issues"][0]["ready"], true);
    assert_eq!(default["issues"][0]["available"], true);
    assert_eq!(default["summary"]["operational_issue_count"], 4);
    assert_eq!(default["summary"]["ready_count"], 3);
    assert_eq!(default["summary"]["executable_count"], 2);
    assert_eq!(default["summary"]["assigned_ready_count"], 1);
    assert_eq!(default["summary"]["blocked_count"], 1);
    assert_eq!(default["warnings"], serde_json::json!([]));

    let assigned = ready_command(&state, &github.url(), Some("alice"))
        .output()
        .expect("run grit ready for assignee");
    assert!(
        assigned.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&assigned.stderr)
    );
    let assigned: Value = serde_json::from_slice(&assigned.stdout).expect("assigned ready JSON");
    assert_eq!(assigned["execution_scope"]["mode"], "assignee");
    assert_eq!(assigned["execution_scope"]["assignee"], "alice");
    assert_eq!(issue_numbers(&assigned), vec![2]);
    assert_eq!(assigned["issues"][0]["ready"], true);
    assert_eq!(assigned["issues"][0]["available"], false);
    assert_eq!(assigned["issues"][0]["assignees"][0], "alice");

    mocks.assert();
}

#[test]
fn ready_falls_back_to_the_latest_valid_replica_without_advancing_synced_at() {
    let mut github = Server::new();
    let mocks = mock_repository(
        &mut github,
        "acme/widgets",
        vec![issue(1, "open", &[], &[])],
        vec![(1, vec![])],
    );
    let api_url = github.url();
    let state = TempDir::new().expect("temporary state directory");

    let online = ready_command(&state, &api_url, None)
        .output()
        .expect("online ready");
    assert!(online.status.success());
    let online: Value = serde_json::from_slice(&online.stdout).expect("online ready JSON");
    let synced_at = online["synced_at"].clone();
    let input_hash = online["input_hash"].clone();
    mocks.assert();
    drop(github);

    let offline = ready_command(&state, &api_url, None)
        .env_remove("GH_TOKEN")
        .output()
        .expect("offline ready");
    assert!(
        offline.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&offline.stderr)
    );
    let offline: Value = serde_json::from_slice(&offline.stdout).expect("offline ready JSON");
    assert_eq!(offline["source"], "local_fallback");
    assert_eq!(offline["synced_at"], synced_at);
    assert_eq!(offline["input_hash"], input_hash);
    assert_eq!(issue_numbers(&offline), vec![1]);
    assert_eq!(offline["warnings"][0]["code"], "offline_fallback");
    assert_eq!(
        offline["warnings"][0]["message"],
        "GitHub refresh failed; using the latest valid Local replica"
    );
}

#[test]
fn ready_respects_cycles_and_and_dependencies_and_every_external_state() {
    let mut github = Server::new();
    let dependencies = (1_u64..=9)
        .map(|number| (number, graph_dependencies(number)))
        .collect();
    let mocks = mock_repository(
        &mut github,
        "acme/graph",
        graph_issue_inventory(),
        dependencies,
    );
    let state = TempDir::new().expect("temporary state directory");

    let output = ready_command_for(&state, &github.url(), "acme/graph", None)
        .output()
        .expect("run grit ready");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: Value = serde_json::from_slice(&output.stdout).expect("ready JSON");
    assert_eq!(issue_numbers(&output), vec![1, 5]);
    assert_eq!(output["summary"]["operational_issue_count"], 8);
    assert_eq!(output["summary"]["ready_count"], 2);
    assert_eq!(output["summary"]["blocked_count"], 6);

    mocks.assert();
}

#[test]
fn ready_fails_clearly_without_github_and_a_valid_replica() {
    let state = TempDir::new().expect("temporary state directory");
    let missing = ready_command(&state, "http://127.0.0.1:1", None)
        .output()
        .expect("ready without GitHub");
    assert!(!missing.status.success());
    let missing_error = String::from_utf8_lossy(&missing.stderr);
    assert!(missing_error.contains("GitHub refresh failed"));
    assert!(missing_error.contains("no valid Local replica"));
    assert!(missing_error.contains("no Local replica exists"));

    let replica_dir = state.path().join("repositories/acme/widgets");
    fs::create_dir_all(&replica_dir).expect("replica directory");
    fs::write(replica_dir.join("replica.json"), "{\"schema_version\":").expect("corrupt replica");
    let corrupt = ready_command(&state, "http://127.0.0.1:1", None)
        .output()
        .expect("ready with corrupt replica");
    assert!(!corrupt.status.success());
    let corrupt_error = String::from_utf8_lossy(&corrupt.stderr);
    assert!(corrupt_error.contains("GitHub refresh failed"));
    assert!(corrupt_error.contains("no valid Local replica"));
    assert!(corrupt_error.contains("could not decode the Local replica"));
}

fn ready_command(state: &TempDir, api_url: &str, assignee: Option<&str>) -> Command {
    ready_command_for(state, api_url, "acme/widgets", assignee)
}

fn ready_command_for(
    state: &TempDir,
    api_url: &str,
    repository: &str,
    assignee: Option<&str>,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["ready", "--repo", repository, "--json"]);
    if let Some(assignee) = assignee {
        command.args(["--assignee", assignee]);
    }
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn issue_numbers(document: &Value) -> Vec<u64> {
    document["issues"]
        .as_array()
        .expect("issues array")
        .iter()
        .map(|issue| issue["number"].as_u64().expect("Issue number"))
        .collect()
}

fn issue_inventory() -> Vec<Value> {
    vec![
        issue(1, "open", &[], &[]),
        issue(2, "open", &[], &["alice"]),
        issue(3, "open", &[], &[]),
        issue(4, "open", &[], &[]),
        issue(5, "closed", &[], &[]),
    ]
}

fn graph_issue_inventory() -> Vec<Value> {
    (1_u64..=9)
        .map(|number| {
            let state = if number == 9 { "closed" } else { "open" };
            issue(number, state, &[], &[])
        })
        .collect()
}

fn graph_dependencies(number: u64) -> Vec<Value> {
    match number {
        2 => vec![internal_blocker(3, "open")],
        3 => vec![internal_blocker(2, "open")],
        4 => vec![internal_blocker(2, "open")],
        5 => vec![external_blocker("partners/platform", 50, "closed")],
        6 => vec![external_blocker("partners/platform", 60, "open")],
        7 => vec![external_blocker("partners/platform", 70, "unknown")],
        8 => vec![
            internal_blocker(1, "open"),
            internal_blocker(1, "open"),
            internal_blocker(9, "closed"),
        ],
        _ => Vec::new(),
    }
}
