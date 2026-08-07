use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
};

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn block_publishes_the_native_edge_and_offline_frontier() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let create = mock_add_response(&mut github, 201);
    let readback = mock_repository(&mut github, &blocker());

    let output = mutation_command(&state, &github.url(), "block")
        .output()
        .expect("block command");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("block JSON");
    assert_mutation_output(&output, "block", "created", 1);
    locator.assert();
    preflight.assert();
    create.assert();
    readback.assert();
    assert_offline_ready(&state, &github.url(), vec![1]);
}

#[test]
fn unblock_publishes_the_removed_edge_and_offline_frontier() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    seed_replica(&mut github, &state, &blocker());
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, &blocker());
    let remove = mock_remove_response(&mut github, 200);
    let readback = mock_repository(&mut github, "[]");

    let output = mutation_command(&state, &github.url(), "unblock")
        .output()
        .expect("unblock command");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("unblock JSON");
    assert_mutation_output(&output, "unblock", "removed", 0);
    locator.assert();
    preflight.assert();
    remove.assert();
    readback.assert();
    assert_offline_ready(&state, &github.url(), vec![1, 2]);
}

#[test]
fn adding_an_existing_edge_is_an_idempotent_human_readable_success() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, &blocker());
    let readback = mock_repository(&mut github, &blocker());

    let output = mutation_command_human(&state, &github.url(), "block")
        .output()
        .expect("idempotent block");
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "acme/widgets#2 was already blocked by acme/widgets#1"
    );
    locator.assert();
    preflight.assert();
    readback.assert();
}

#[test]
fn removing_an_absent_edge_is_an_idempotent_json_success() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let readback = mock_repository(&mut github, "[]");

    let output = mutation_command(&state, &github.url(), "unblock")
        .output()
        .expect("idempotent unblock");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("unblock JSON");
    assert_mutation_output(&output, "unblock", "already_absent", 0);
    locator.assert();
    preflight.assert();
    readback.assert();
}

#[test]
fn a_rejected_add_leaves_the_replica_unchanged() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut github, &state, "[]");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let rejected = mock_add_response(&mut github, 403);

    let output = mutation_command(&state, &github.url(), "block")
        .output()
        .expect("rejected block");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP 403"));
    assert_replica_unchanged(&state, &before);
    locator.assert();
    preflight.assert();
    rejected.assert();
}

#[test]
fn an_ambiguous_add_response_leaves_the_replica_unchanged() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut github, &state, "[]");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let uncertain = mock_add_response(&mut github, 500);

    let output = mutation_command(&state, &github.url(), "block")
        .output()
        .expect("uncertain block response");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("outcome of adding the blocked-by relationship is uncertain")
    );
    assert_replica_unchanged(&state, &before);
    locator.assert();
    preflight.assert();
    uncertain.assert();
}

#[test]
fn an_ambiguous_remove_response_leaves_the_replica_unchanged() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut github, &state, &blocker());
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, &blocker());
    let uncertain = mock_remove_response(&mut github, 500);

    let output = mutation_command(&state, &github.url(), "unblock")
        .output()
        .expect("uncertain unblock response");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("outcome of removing the blocked-by relationship is uncertain")
    );
    assert_replica_unchanged(&state, &before);
    locator.assert();
    preflight.assert();
    uncertain.assert();
}

#[test]
fn failed_synchronized_readback_leaves_the_replica_unchanged() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut github, &state, "[]");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let accepted = mock_add_response(&mut github, 201);
    let failed_readback = mock_failed_inventory(&mut github, 503);

    let output = mutation_command(&state, &github.url(), "block")
        .output()
        .expect("block with failed readback");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GitHub dependency operation completed"));
    assert!(stderr.contains("Local replica was not changed"));
    assert_replica_unchanged(&state, &before);
    locator.assert();
    preflight.assert();
    accepted.assert();
    failed_readback.assert();
}

#[test]
fn a_transport_interruption_reports_uncertainty_and_preserves_the_replica() {
    let mut baseline_github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut baseline_github, &state, "[]");
    let (api_url, server) = uncertain_write_server();

    let output = mutation_command(&state, &api_url, "block")
        .output()
        .expect("block with uncertain write");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("outcome of adding the blocked-by relationship is uncertain"));
    assert!(stderr.contains("Local replica was not changed"));
    assert_replica_unchanged(&state, &before);
    server.join().expect("uncertain-write server");
}

#[test]
fn a_concurrent_add_is_reconciled_as_already_present() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let concurrent = mock_add_response(&mut github, 422);
    let reconciliation = mock_reconciliation(&mut github, &blocker());
    let readback = mock_repository(&mut github, &blocker());

    let output = mutation_command(&state, &github.url(), "block")
        .output()
        .expect("concurrent block");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("block JSON");
    assert_mutation_output(&output, "block", "already_present", 1);
    locator.assert();
    preflight.assert();
    concurrent.assert();
    reconciliation.assert();
    readback.assert();
}

#[test]
fn a_concurrent_remove_is_reconciled_as_already_absent() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, &blocker());
    let concurrent = mock_remove_response(&mut github, 404);
    let reconciliation = mock_reconciliation(&mut github, "[]");
    let readback = mock_repository(&mut github, "[]");

    let output = mutation_command(&state, &github.url(), "unblock")
        .output()
        .expect("concurrent unblock");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("unblock JSON");
    assert_mutation_output(&output, "unblock", "already_absent", 0);
    locator.assert();
    preflight.assert();
    concurrent.assert();
    reconciliation.assert();
    readback.assert();
}

#[test]
fn dependency_references_reject_ambiguous_or_zero_issue_forms_before_authentication() {
    let state = TempDir::new().expect("temporary state directory");
    for reference in ["42", "acme/widgets", "acme/widgets#0", "acme/widgets#2#3"] {
        let output = Command::new(env!("CARGO_BIN_EXE_grit"))
            .args(["block", reference, "--by", "acme/widgets#1", "--json"])
            .env_remove("GH_TOKEN")
            .env("GRIT_STATE_DIR", state.path())
            .env("PATH", "")
            .output()
            .expect("invalid Issue reference");
        assert!(!output.status.success(), "accepted {reference}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("OWNER/REPO#NUMBER"),
            "stderr for {reference}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn assert_mutation_output(output: &Value, command: &str, result: &str, dependency_count: u64) {
    assert_eq!(output["schema_version"], "grit.dependency-mutation/v1");
    assert_eq!(output["command"], command);
    assert_eq!(output["repository"], "acme/widgets");
    assert_eq!(output["result"], result);
    assert_eq!(
        output["edge"],
        json!({
            "blocked": "acme/widgets#2",
            "blocker": "acme/widgets#1",
            "kind": "blocked_by"
        })
    );
    assert_eq!(output["snapshot"]["dependency_count"], dependency_count);
    assert!(output["snapshot"]["synced_at"].as_str().is_some());
    assert!(output["snapshot"]["input_hash"].as_str().is_some());
}

fn mutation_command(state: &TempDir, api_url: &str, command_name: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args([
        command_name,
        "acme/widgets#2",
        "--by",
        "acme/widgets#1",
        "--json",
    ]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn mutation_command_human(state: &TempDir, api_url: &str, command_name: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args([command_name, "acme/widgets#2", "--by", "acme/widgets#1"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn ready_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["ready", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn sync_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["sync", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn mock_blocker_locator(github: &mut Server) -> Mock {
    github
        .mock("GET", "/repos/acme/widgets/issues/1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"id": 100, "number": 1, "pull_request": null}).to_string())
        .create()
}

fn mock_preflight(github: &mut Server, body: &str) -> Mock {
    github
        .mock(
            "GET",
            "/repos/acme/widgets/issues/2/dependencies/blocked_by",
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "50".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create()
}

fn mock_add_response(github: &mut Server, status: usize) -> Mock {
    github
        .mock(
            "POST",
            "/repos/acme/widgets/issues/2/dependencies/blocked_by",
        )
        .match_body(Matcher::Json(json!({"issue_id": 100})))
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(issue(2).to_string())
        .create()
}

fn mock_remove_response(github: &mut Server, status: usize) -> Mock {
    github
        .mock(
            "DELETE",
            "/repos/acme/widgets/issues/2/dependencies/blocked_by/100",
        )
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(issue(2).to_string())
        .create()
}

struct FailedInventoryMocks {
    labels: Mock,
    issues: Mock,
}

impl FailedInventoryMocks {
    fn assert(self) {
        self.labels.assert();
        self.issues.assert();
    }
}

fn mock_failed_inventory(github: &mut Server, status: usize) -> FailedInventoryMocks {
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let issues = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(json!({"message": "unavailable"}).to_string())
        .create();
    FailedInventoryMocks { labels, issues }
}

fn mock_reconciliation(github: &mut Server, body: &str) -> Mock {
    github
        .mock(
            "GET",
            "/repos/acme/widgets/issues/2/dependencies/blocked_by",
        )
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("per_page".into(), "50".into()),
            Matcher::UrlEncoded("page".into(), "1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create()
}

fn seed_replica(github: &mut Server, state: &TempDir, blockers: &str) -> Vec<u8> {
    let baseline = mock_repository(github, blockers);
    let synchronized = sync_command(state, &github.url())
        .output()
        .expect("baseline sync");
    assert_success(&synchronized);
    baseline.assert();
    fs::read(replica_path(state)).expect("baseline replica")
}

fn assert_replica_unchanged(state: &TempDir, before: &[u8]) {
    assert_eq!(
        fs::read(replica_path(state)).expect("unchanged replica"),
        before
    );
}

fn replica_path(state: &TempDir) -> std::path::PathBuf {
    state.path().join("repositories/acme/widgets/replica.json")
}

fn assert_offline_ready(state: &TempDir, api_url: &str, expected: Vec<u64>) {
    let output = ready_command(state, api_url)
        .env_remove("GH_TOKEN")
        .output()
        .expect("offline ready");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("ready JSON");
    assert_eq!(issue_numbers(&output), expected);
}

struct RepositoryMocks {
    labels: Mock,
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
}

impl RepositoryMocks {
    fn assert(self) {
        self.labels.assert();
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
    }
}

fn mock_repository(github: &mut Server, issue_two_blockers: &str) -> RepositoryMocks {
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let issues = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!([issue(1), issue(2)]).to_string())
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependencies = vec![
        github
            .mock(
                "GET",
                "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            )
            .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .create(),
        github
            .mock(
                "GET",
                "/repos/acme/widgets/issues/2/dependencies/blocked_by",
            )
            .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issue_two_blockers)
            .create(),
    ];
    RepositoryMocks {
        labels,
        issues,
        comments,
        dependencies,
    }
}

fn issue(number: u64) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": "open",
        "state_reason": null,
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": null,
        "assignees": [],
        "labels": [],
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": null
    })
}

fn blocker() -> String {
    json!([{
        "id": 100,
        "node_id": "I_1",
        "repository_url": "https://api.github.com/repos/acme/widgets",
        "number": 1,
        "state": "open"
    }])
    .to_string()
}

fn issue_numbers(document: &Value) -> Vec<u64> {
    document["issues"]
        .as_array()
        .expect("issues array")
        .iter()
        .map(|issue| issue["number"].as_u64().expect("Issue number"))
        .collect()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn uncertain_write_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("uncertain-write listener");
    let address = listener.local_addr().expect("listener address");
    let server = thread::spawn(move || {
        reply_once(
            &listener,
            &json!({"id": 100, "number": 1, "pull_request": null}).to_string(),
        );
        reply_once(&listener, "[]");
        let (mut stream, _) = listener.accept().expect("mutation connection");
        read_request_headers(&mut stream);
    });
    (format!("http://{address}"), server)
}

fn reply_once(listener: &TcpListener, body: &str) {
    let (mut stream, _) = listener.accept().expect("HTTP connection");
    read_request_headers(&mut stream);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .expect("HTTP response");
}

fn read_request_headers(stream: &mut impl Read) {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).expect("HTTP request");
        if read == 0 {
            break;
        }
        request.push(byte[0]);
    }
}
