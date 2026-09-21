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
fn offline_block_is_projected_into_next_without_changing_the_replica() {
    let mut online = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut online, &state, "[]");

    let mut unavailable = Server::new();
    let failed_locator = unavailable
        .mock("GET", "/repos/acme/widgets/issues/2")
        .with_status(503)
        .expect(1)
        .create();
    let failed_refresh = unavailable
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(503)
        .expect(1)
        .create();
    let forbidden_write = unavailable
        .mock(
            "POST",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
        )
        .expect(0)
        .create();

    let queued = dependency_command(
        &state,
        &unavailable.url(),
        "block",
        "acme/widgets#1",
        "acme/widgets#2",
    )
    .output()
    .expect("queue offline dependency");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("queued dependency JSON");
    assert_eq!(queued["result"], "pending");
    assert_eq!(queued["pending"], true);
    assert_eq!(queued["operation"]["kind"], "dependency_update");
    assert_eq!(queued["operation"]["desired_present"], true);
    let operation_id = queued["operation"]["id"]
        .as_str()
        .expect("operation ID")
        .to_owned();

    let next = next_command(&state, &unavailable.url())
        .output()
        .expect("rank projected dependency");
    assert_success(&next);
    let next: Value = serde_json::from_slice(&next.stdout).expect("projected next JSON");
    assert_eq!(next["source"], "local_fallback");
    assert_eq!(next["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(next["pending"], true);
    assert_eq!(next["pending_operation_ids"], json!([operation_id]));
    assert_eq!(next["recommendation"]["pending"], true);
    assert_eq!(next["recommendation"]["first_issue"]["pending"], true);
    assert_eq!(
        next["recommendation"]["first_issue"]["operation_ids"],
        next["pending_operation_ids"]
    );
    assert_eq!(next["recommendation"]["reasons"][0]["pending"], true);
    assert_eq!(
        next["recommendation"]["operation_ids"],
        next["pending_operation_ids"]
    );

    assert_replica_unchanged(&state, &before);
    failed_locator.assert();
    failed_refresh.assert();
    forbidden_write.assert();
}

#[test]
fn external_offline_blocker_is_opaque_and_marks_the_unrelated_new_winner() {
    let mut online = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut online, &state, "[]");

    let mut unavailable = Server::new();
    let failed_locator = unavailable
        .mock("GET", "/repos/partners/api/issues/9")
        .with_status(503)
        .expect(1)
        .create();
    let failed_refresh = unavailable
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(503)
        .expect(1)
        .create();

    let queued = dependency_command(
        &state,
        &unavailable.url(),
        "block",
        "acme/widgets#1",
        "partners/api#9",
    )
    .output()
    .expect("queue external blocker");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("queued dependency JSON");
    let operation_id = queued["operation"]["id"]
        .as_str()
        .expect("operation ID")
        .to_owned();

    let next = next_command(&state, &unavailable.url())
        .output()
        .expect("rank with opaque external blocker");
    assert_success(&next);
    let next: Value = serde_json::from_slice(&next.stdout).expect("next JSON");
    assert_eq!(next["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(next["recommendation"]["first_issue"]["pending"], false);
    assert_eq!(next["recommendation"]["pending"], true);
    assert_eq!(
        next["recommendation"]["operation_ids"],
        json!([operation_id])
    );
    assert_eq!(next["recommendation"]["reasons"][0]["pending"], true);
    assert_eq!(next["summary"]["unknown_blocker_count"], 1);

    assert_replica_unchanged(&state, &before);
    failed_locator.assert();
    failed_refresh.assert();
}

#[test]
fn queued_external_dependency_reconciles_then_publishes_verified_readback() {
    let mut online = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut online, &state, "[]");
    let mut unavailable = Server::new();
    let failed_locator = unavailable
        .mock("GET", "/repos/partners/api/issues/9")
        .with_status(503)
        .expect(1)
        .create();
    let queued = dependency_command(
        &state,
        &unavailable.url(),
        "block",
        "acme/widgets#1",
        "partners/api#9",
    )
    .output()
    .expect("queue dependency");
    assert_success(&queued);
    assert_replica_unchanged(&state, &before);
    failed_locator.assert();

    let mut github = Server::new();
    let preflight = mock_repository_for_edges(&mut github, "[]", "[]");
    let locator = github
        .mock("GET", "/repos/partners/api/issues/9")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"id": 900, "number": 9, "pull_request": null}).to_string())
        .create();
    let edge_preflight = github
        .mock(
            "GET",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "50".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let create = github
        .mock(
            "POST",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
        )
        .match_body(Matcher::Json(json!({"issue_id": 900})))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(issue(1).to_string())
        .create();
    let readback = mock_repository_for_edges(
        &mut github,
        &external_blocker_for("partners/api", 9, 900),
        "[]",
    );

    let reconciled = reconcile_command(&state, &github.url())
        .output()
        .expect("reconcile queued dependency");
    assert_success(&reconciled);
    let reconciled: Value =
        serde_json::from_slice(&reconciled.stdout).expect("reconciliation JSON");
    assert_eq!(reconciled["operations"][0]["kind"], "dependency_update");
    assert_eq!(reconciled["operations"][0]["classification"], "applicable");
    assert_eq!(reconciled["operations"][0]["outcome"], "applied");
    assert_eq!(reconciled["operations"][0]["desired_present"], true);
    assert_eq!(
        reconciled["operations"][0]["edge"]["blocker_repository"],
        "partners/api"
    );
    assert_eq!(reconciled["summary"]["remaining"], 0);
    assert_eq!(reconciled["snapshot"]["dependency_count"], 1);

    preflight.assert();
    locator.assert();
    edge_preflight.assert();
    create.assert();
    readback.assert();
}

#[test]
fn dropped_dependency_response_is_retired_by_set_semantics_without_replaying_the_write() {
    let mut online = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    seed_replica(&mut online, &state, "[]");
    let mut unavailable = Server::new();
    let failed_locator = unavailable
        .mock("GET", "/repos/acme/widgets/issues/2")
        .with_status(503)
        .expect(1)
        .create();
    let queued = dependency_command(
        &state,
        &unavailable.url(),
        "block",
        "acme/widgets#1",
        "acme/widgets#2",
    )
    .output()
    .expect("queue dependency");
    assert_success(&queued);
    failed_locator.assert();

    let (uncertain_url, uncertain_server) = dropped_reconciliation_server();

    let first = reconcile_command(&state, &uncertain_url)
        .output()
        .expect("uncertain first reconciliation");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("first reconciliation JSON");
    assert_eq!(first["operations"][0]["outcome"], "failed");
    assert_eq!(first["summary"]["remaining"], 1);
    assert_eq!(first["snapshot"]["dependency_count"], 1);
    uncertain_server
        .join()
        .expect("dropped-response reconciliation server");

    let mut retry = Server::new();
    let retry_preflight = mock_repository_for_edges(&mut retry, &blocker_for(2), "[]");
    let forbidden_replay = retry
        .mock(
            "POST",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
        )
        .expect(0)
        .create();
    let second = reconcile_command(&state, &retry.url())
        .output()
        .expect("set-semantic retry");
    assert_success(&second);
    let second: Value = serde_json::from_slice(&second.stdout).expect("second reconciliation JSON");
    assert_eq!(
        second["operations"][0]["classification"],
        "already_satisfied"
    );
    assert_eq!(second["operations"][0]["outcome"], "already_satisfied");
    assert_eq!(second["summary"]["remaining"], 0);
    retry_preflight.assert();
    forbidden_replay.assert();
}

#[test]
fn failed_dependency_prerequisite_blocks_only_its_dependent_branch() {
    let mut online = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    seed_replica(&mut online, &state, "[]");
    let mut unavailable = Server::new();
    let failed_blocker_reads = unavailable
        .mock("GET", "/repos/acme/widgets/issues/2")
        .with_status(503)
        .expect(2)
        .create();
    let failed_priority_read = unavailable
        .mock("GET", "/repos/acme/widgets/issues/1")
        .with_status(503)
        .expect(1)
        .create();
    for command_name in ["block", "unblock"] {
        let queued = dependency_command(
            &state,
            &unavailable.url(),
            command_name,
            "acme/widgets#1",
            "acme/widgets#2",
        )
        .output()
        .expect("queue ordered dependency intent");
        assert_success(&queued);
    }
    let priority = priority_command(&state, &unavailable.url(), 1, "p0")
        .output()
        .expect("queue independent Priority intent");
    assert_success(&priority);
    failed_blocker_reads.assert();
    failed_priority_read.assert();

    let mut github = Server::new();
    let preflight = mock_repository_for_edges(&mut github, "[]", "[]");
    let locator = github
        .mock("GET", "/repos/acme/widgets/issues/2")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"id": 200, "number": 2, "pull_request": null}).to_string())
        .create();
    let edge_preflight = github
        .mock(
            "GET",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "50".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let rejected = github
        .mock(
            "POST",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
        )
        .with_status(403)
        .create();
    let independent = github
        .mock("POST", "/repos/acme/widgets/issues/1/labels")
        .match_body(Matcher::Json(json!({"labels": ["priority:p0"]})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let readback = mock_repository_state(&mut github, "[]", "[]", &["priority:p0"], &[]);

    let output = reconcile_command(&state, &github.url())
        .output()
        .expect("reconcile branched mutations");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("reconciliation JSON");
    let operations = output["operations"].as_array().expect("operations");
    assert_eq!(operations[0]["outcome"], "failed");
    assert_eq!(operations[1]["outcome"], "transitively_blocked");
    assert_eq!(operations[1]["blocked_by"], json!([operations[0]["id"]]));
    assert_eq!(operations[2]["kind"], "priority_update");
    assert_eq!(operations[2]["outcome"], "applied");
    assert_eq!(output["summary"]["remaining"], 2);

    preflight.assert();
    locator.assert();
    edge_preflight.assert();
    rejected.assert();
    independent.assert();
    readback.assert();
}

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
fn an_ambiguous_add_response_is_queued_and_leaves_the_replica_unchanged() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut github, &state, "[]");
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, "[]");
    let uncertain = mock_add_response(&mut github, 500);

    let output = mutation_command(&state, &github.url(), "block")
        .output()
        .expect("uncertain block response");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("Pending block JSON");
    assert_eq!(output["result"], "pending");
    assert_eq!(output["pending"], true);
    assert_eq!(output["operation"]["desired_present"], true);
    assert_replica_unchanged(&state, &before);
    locator.assert();
    preflight.assert();
    uncertain.assert();
}

#[test]
fn an_ambiguous_remove_response_is_queued_and_leaves_the_replica_unchanged() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut github, &state, &blocker());
    let locator = mock_blocker_locator(&mut github);
    let preflight = mock_preflight(&mut github, &blocker());
    let uncertain = mock_remove_response(&mut github, 500);

    let output = mutation_command(&state, &github.url(), "unblock")
        .output()
        .expect("uncertain unblock response");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("Pending unblock JSON");
    assert_eq!(output["result"], "pending");
    assert_eq!(output["pending"], true);
    assert_eq!(output["operation"]["desired_present"], false);
    assert_replica_unchanged(&state, &before);
    assert_offline_ready(&state, &github.url(), vec![1, 2]);
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
fn a_transport_interruption_queues_the_intent_and_preserves_the_replica() {
    let mut baseline_github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let before = seed_replica(&mut baseline_github, &state, "[]");
    let (api_url, server) = uncertain_write_server();

    let output = mutation_command(&state, &api_url, "block")
        .output()
        .expect("block with uncertain write");
    assert_success(&output);
    let output: Value =
        serde_json::from_slice(&output.stdout).expect("Pending interrupted block JSON");
    assert_eq!(output["result"], "pending");
    assert_eq!(output["pending"], true);
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
        let output = Command::new(env!("CARGO_BIN_EXE_hyfa"))
            .args(["block", reference, "--by", "acme/widgets#1", "--json"])
            .env_remove("GH_TOKEN")
            .env("HYFA_NO_KEYRING", "1")
            .env("HYFA_STATE_DIR", state.path())
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
    assert_eq!(output["schema_version"], "hyfa.dependency-mutation/v1");
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
    dependency_command(
        state,
        api_url,
        command_name,
        "acme/widgets#2",
        "acme/widgets#1",
    )
}

fn dependency_command(
    state: &TempDir,
    api_url: &str,
    command_name: &str,
    blocked: &str,
    blocker: &str,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args([command_name, blocked, "--by", blocker, "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn next_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args(["next", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn reconcile_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args(["reconcile", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn priority_command(state: &TempDir, api_url: &str, issue_number: u64, priority: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args([
        "update",
        &format!("acme/widgets#{issue_number}"),
        "--priority",
        priority,
        "--json",
    ]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn mutation_command_human(state: &TempDir, api_url: &str, command_name: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args([command_name, "acme/widgets#2", "--by", "acme/widgets#1"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn ready_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args(["ready", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn sync_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args(["sync", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
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
    events: Mock,
    labels: Mock,
    issues: Mock,
}

impl FailedInventoryMocks {
    fn assert(self) {
        self.events.assert();
        self.labels.assert();
        self.issues.assert();
    }
}

fn mock_failed_inventory(github: &mut Server, status: usize) -> FailedInventoryMocks {
    let (labels, events) = mock_sync_metadata(github);
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
    FailedInventoryMocks {
        labels,
        events,
        issues,
    }
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
    events: Mock,
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
}

impl RepositoryMocks {
    fn assert(self) {
        self.labels.assert();
        self.events.assert();
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
    }
}

fn mock_repository(github: &mut Server, issue_two_blockers: &str) -> RepositoryMocks {
    mock_repository_for_edges(github, "[]", issue_two_blockers)
}

fn mock_repository_for_edges(
    github: &mut Server,
    issue_one_blockers: &str,
    issue_two_blockers: &str,
) -> RepositoryMocks {
    mock_repository_state(github, issue_one_blockers, issue_two_blockers, &[], &[])
}

fn mock_repository_state(
    github: &mut Server,
    issue_one_blockers: &str,
    issue_two_blockers: &str,
    issue_one_labels: &[&str],
    issue_two_labels: &[&str],
) -> RepositoryMocks {
    let (labels, events) = mock_sync_metadata(github);
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
        .with_body(
            json!([
                issue_with_labels(1, issue_one_labels),
                issue_with_labels(2, issue_two_labels)
            ])
            .to_string(),
        )
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
            .with_body(issue_one_blockers)
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
        events,
        issues,
        comments,
        dependencies,
    }
}

fn issue(number: u64) -> Value {
    issue_with_labels(number, &[])
}

fn issue_with_labels(number: u64, labels: &[&str]) -> Value {
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
        "labels": labels
            .iter()
            .enumerate()
            .map(|(index, name)| json!({
                "id": number * 10 + index as u64,
                "node_id": format!("L_{number}_{index}"),
                "name": name,
                "color": "123456",
                "description": null
            }))
            .collect::<Vec<_>>(),
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": null
    })
}

fn blocker() -> String {
    blocker_for(1)
}

fn blocker_for(number: u64) -> String {
    external_blocker_for("acme/widgets", number, number * 100)
}

fn external_blocker_for(repository: &str, number: u64, id: u64) -> String {
    json!([{
        "id": id,
        "node_id": format!("I_{number}"),
        "repository_url": format!("https://api.github.com/repos/{repository}"),
        "number": number,
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

fn dropped_reconciliation_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reconciliation listener");
    let address = listener.local_addr().expect("listener address");
    let issues = json!([issue(1), issue(2)]).to_string();
    let locator = json!({"id": 200, "number": 2, "pull_request": null}).to_string();
    let applied_edge = blocker_for(2);
    let steps = vec![
        ("GET", "/repos/acme/widgets/labels", Some("[]".to_owned())),
        (
            "GET",
            "/repos/acme/widgets/issues/events",
            Some("[]".to_owned()),
        ),
        ("GET", "/repos/acme/widgets/issues", Some(issues.clone())),
        (
            "GET",
            "/repos/acme/widgets/issues/comments",
            Some("[]".to_owned()),
        ),
        (
            "GET",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            Some("[]".to_owned()),
        ),
        (
            "GET",
            "/repos/acme/widgets/issues/2/dependencies/blocked_by",
            Some("[]".to_owned()),
        ),
        ("GET", "/repos/acme/widgets/issues/2", Some(locator)),
        (
            "GET",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            Some("[]".to_owned()),
        ),
        (
            "POST",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            None,
        ),
        ("GET", "/repos/acme/widgets/labels", Some("[]".to_owned())),
        (
            "GET",
            "/repos/acme/widgets/issues/events",
            Some("[]".to_owned()),
        ),
        ("GET", "/repos/acme/widgets/issues", Some(issues)),
        (
            "GET",
            "/repos/acme/widgets/issues/comments",
            Some("[]".to_owned()),
        ),
        (
            "GET",
            "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            Some(applied_edge),
        ),
        (
            "GET",
            "/repos/acme/widgets/issues/2/dependencies/blocked_by",
            Some("[]".to_owned()),
        ),
    ];
    let server = thread::spawn(move || {
        for (expected_method, expected_path, response) in steps {
            let (mut stream, _) = listener.accept().expect("reconciliation connection");
            let (method, path) = read_http_request(&mut stream);
            assert_eq!(method, expected_method);
            assert_eq!(path, expected_path);
            if let Some(body) = response {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("HTTP response");
            }
        }
    });
    (format!("http://{address}"), server)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> (String, String) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("HTTP request bytes");
        assert!(read > 0, "request ended before headers");
        request.extend_from_slice(&buffer[..read]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("HTTP request body");
        assert!(read > 0, "request ended before body");
        request.extend_from_slice(&buffer[..read]);
    }
    let request_line = headers.lines().next().expect("request line");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().expect("request method").to_owned();
    let target = parts.next().expect("request target");
    let path = target.split('?').next().expect("request path").to_owned();
    (method, path)
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

fn mock_sync_metadata(github: &mut Server) -> (Mock, Mock) {
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    (labels, events)
}
