use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn offline_priority_survives_restart_and_changes_analysis_without_remote_writes() {
    let fixture = synchronized_fixture();
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 1, "priority:p1", 1, 2);

    let queued = update_command(&fixture.state, &unavailable.url(), 1, "p0")
        .output()
        .expect("queue offline P0");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("pending update JSON");
    assert_eq!(queued["status"], "pending");
    assert_eq!(
        queued["previous_priority"],
        json!({"state": "declared", "comparison": "p1", "value": "p1"})
    );
    assert_eq!(
        queued["resulting_priority"],
        json!({"state": "declared", "comparison": "p0", "value": "p0"})
    );
    assert_eq!(
        queued["operation"]["base"],
        json!({"state": "declared", "value": "p1"})
    );
    assert_eq!(
        queued["operation"]["desired"],
        json!({"state": "declared", "value": "p0"})
    );
    let operation_id = queued["operation"]["id"]
        .as_str()
        .expect("operation ID")
        .to_owned();
    assert_ne!(
        queued["working_graph"]["input_hash"],
        fixture.replica_json["input_hash"]
    );

    let provisional = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank projected P0 after process restart");
    assert_success(&provisional);
    let provisional: Value =
        serde_json::from_slice(&provisional.stdout).expect("provisional next JSON");
    assert_eq!(provisional["source"], "local_fallback");
    assert_eq!(provisional["synced_at"], fixture.replica_json["synced_at"]);
    assert_eq!(
        provisional["replica_snapshot_hash"],
        fixture.replica_json["input_hash"]
    );
    assert_eq!(provisional["pending"], true);
    assert_eq!(provisional["pending_operation_ids"], json!([operation_id]));
    assert_eq!(provisional["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(provisional["recommendation"]["pending"], true);
    assert_eq!(
        provisional["recommendation"]["operation_ids"],
        provisional["pending_operation_ids"]
    );
    assert_eq!(
        provisional["recommendation"]["first_issue"]["pending"],
        true
    );
    assert_eq!(provisional["comparison_to_runner_up"]["pending"], true);
    assert_eq!(provisional["recommendation"]["reasons"][0]["pending"], true);

    let ready = ready_command(&fixture.state, &unavailable.url())
        .output()
        .expect("inspect projected Ready Issues after process restart");
    assert_success(&ready);
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("provisional ready JSON");
    assert_eq!(ready["pending"], true);
    assert_eq!(ready["issues"][0]["number"], 1);
    assert_eq!(ready["issues"][0]["pending"], true);
    assert_eq!(ready["issues"][0]["operation_ids"], json!([operation_id]));
    assert_eq!(ready["issues"][0]["priority"]["value"], "p0");

    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

#[test]
fn offline_priority_orders_later_rollout_steps_and_preserves_operation_provenance() {
    let fixture = synchronized_fixture_with(
        &[(1, "priority:p1"), (2, "priority:p3"), (3, "priority:p4")],
        1,
    );
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 3, "priority:p4", 2, 3);

    let first = update_command(&fixture.state, &unavailable.url(), 3, "p2")
        .output()
        .expect("queue later-step P2");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("first operation");
    let after_first = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank first projected rollout");
    assert_success(&after_first);
    let after_first: Value = serde_json::from_slice(&after_first.stdout).expect("first rollout");

    let second = update_command(&fixture.state, &unavailable.url(), 3, "p1")
        .output()
        .expect("queue later-step P1");
    assert_success(&second);
    let second: Value = serde_json::from_slice(&second.stdout).expect("second operation");
    let outbox_path = fixture
        .state
        .path()
        .join("repositories/acme/offline/outbox.json");
    let outbox_before = fs::read(&outbox_path).expect("queued mutations");
    let ranked = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank final projected rollout");
    let repeated = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("repeat after process restart");
    assert_success(&ranked);
    assert_success(&repeated);
    assert_eq!(ranked.stdout, repeated.stdout);
    let ranked: Value = serde_json::from_slice(&ranked.stdout).expect("final rollout");
    let operation_ids = json!([first["operation"]["id"], second["operation"]["id"]]);
    assert_eq!(ranked["parameters"]["horizon"], 3);
    assert_eq!(ranked["mode"], "normal");
    assert_eq!(ranked["search_complete"], true);
    assert_eq!(ranked["global_optimum_claimed"], true);
    assert_eq!(ranked["runner_up_scope"], "global");
    assert_eq!(ranked["truncated_by"], json!([]));
    assert_ne!(ranked["input_hash"], after_first["input_hash"]);
    assert_eq!(ranked["pending_operation_ids"], operation_ids);
    assert_eq!(ranked["recommendation"]["operation_ids"], operation_ids);
    let steps = ranked["recommendation"]["rollout"]["steps"]
        .as_array()
        .expect("three-step rollout");
    assert_eq!(
        steps
            .iter()
            .map(|step| step["issue"]["number"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1), json!(3), json!(2)]
    );
    assert_eq!(steps[0]["issue"]["pending"], false);
    assert_eq!(steps[1]["issue"]["priority"]["value"], "p1");
    assert_eq!(steps[1]["issue"]["operation_ids"], operation_ids);
    assert_eq!(
        ranked["comparison_to_runner_up"]["operation_ids"],
        operation_ids
    );
    assert_eq!(
        fs::read(outbox_path).expect("unchanged mutations"),
        outbox_before
    );
    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

#[test]
fn ordered_concrete_then_none_updates_project_deterministically_across_restarts() {
    let fixture = synchronized_fixture();
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 1, "priority:p1", 2, 1);

    let queued_p0 = update_command(&fixture.state, &unavailable.url(), 1, "p0")
        .output()
        .expect("queue offline P0");
    assert_success(&queued_p0);
    let queued_none = update_command(&fixture.state, &unavailable.url(), 1, "none")
        .output()
        .expect("queue offline none after process restart");
    assert_success(&queued_none);
    let queued_none: Value =
        serde_json::from_slice(&queued_none.stdout).expect("second pending update JSON");
    assert_eq!(
        queued_none["operation"]["base"],
        json!({"state": "declared", "value": "p0"})
    );
    assert_eq!(
        queued_none["operation"]["desired"],
        json!({"state": "unspecified"})
    );

    let outbox: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .state
                .path()
                .join("repositories/acme/offline/outbox.json"),
        )
        .expect("durable outbox"),
    )
    .expect("outbox JSON");
    assert_eq!(outbox["schema_version"], "hyfa.pending-mutations/v1");
    assert_eq!(outbox["repository"], "acme/offline");
    assert_eq!(
        outbox["operations"].as_array().expect("operations").len(),
        2
    );
    assert_eq!(
        outbox["operations"][0]["base"],
        json!({"state": "declared", "value": "p1"})
    );
    assert_eq!(
        outbox["operations"][0]["desired"],
        json!({"state": "declared", "value": "p0"})
    );
    assert_eq!(
        outbox["operations"][1]["base"],
        json!({"state": "declared", "value": "p0"})
    );
    assert_eq!(
        outbox["operations"][1]["desired"],
        json!({"state": "unspecified"})
    );

    let projected = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank ordered concrete then none overlay");
    assert_success(&projected);
    let projected: Value = serde_json::from_slice(&projected.stdout).expect("projected next JSON");
    assert_eq!(projected["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(projected["pending"], true);
    assert_eq!(
        projected["pending_operation_ids"]
            .as_array()
            .expect("operation IDs")
            .len(),
        2
    );

    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

#[test]
fn pending_runner_marks_recommendation_while_normal_mode_stays_stable() {
    let fixture = synchronized_fixture_with(&[(1, "priority:p1"), (2, "priority:p3")], 1);
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 2, "priority:p3", 1, 1);

    let queued = update_command(&fixture.state, &unavailable.url(), 2, "p4")
        .output()
        .expect("queue runner demotion");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("pending update JSON");
    let operation_id = queued["operation"]["id"]
        .as_str()
        .expect("operation ID")
        .to_owned();

    let ranked = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank with a pending runner");
    assert_success(&ranked);
    let ranked: Value = serde_json::from_slice(&ranked.stdout).expect("pending next JSON");
    assert_eq!(ranked["mode"], "normal");
    assert_eq!(ranked["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(ranked["comparison_to_runner_up"]["runner_up"]["number"], 2);
    assert_eq!(ranked["recommendation"]["first_issue"]["pending"], false);
    assert_eq!(ranked["recommendation"]["pending"], true);
    assert_eq!(
        ranked["recommendation"]["operation_ids"],
        json!([operation_id])
    );
    assert_eq!(ranked["recommendation"]["reasons"], json!([]));
    assert_eq!(
        ranked["comparison_to_runner_up"]["operation_ids"],
        ranked["recommendation"]["operation_ids"]
    );
    assert_eq!(ranked["comparison_to_runner_up"]["pending"], true);

    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

#[test]
fn pending_excluded_p0_gate_input_marks_all_current_p0_results() {
    let fixture = synchronized_fixture_with(
        &[
            (1, "priority:p0"),
            (2, "priority:p0"),
            (3, "priority:p0"),
            (4, "priority:p1"),
        ],
        1,
    );
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 1, "priority:p0", 1, 1);

    let queued = update_command(&fixture.state, &unavailable.url(), 1, "p4")
        .output()
        .expect("queue P0-gate demotion");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("pending update JSON");
    let operation_id = queued["operation"]["id"]
        .as_str()
        .expect("operation ID")
        .to_owned();

    let ranked = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank after excluding the former P0 winner");
    assert_success(&ranked);
    let ranked: Value = serde_json::from_slice(&ranked.stdout).expect("pending next JSON");
    assert_eq!(ranked["mode"], "p0_ready");
    assert_eq!(ranked["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(ranked["comparison_to_runner_up"]["runner_up"]["number"], 3);
    assert_eq!(ranked["recommendation"]["first_issue"]["pending"], false);
    assert_eq!(ranked["recommendation"]["pending"], true);
    assert_eq!(
        ranked["recommendation"]["operation_ids"],
        json!([operation_id])
    );
    assert_eq!(ranked["recommendation"]["reasons"][0]["pending"], true);
    assert_eq!(ranked["alternatives"][0]["first_issue"]["number"], 3);
    assert_eq!(ranked["alternatives"][0]["first_issue"]["pending"], false);
    assert_eq!(ranked["alternatives"][0]["pending"], true);
    assert_eq!(
        ranked["alternatives"][0]["operation_ids"],
        json!([operation_id])
    );
    assert_eq!(ranked["alternatives"][0]["reasons"][0]["pending"], true);

    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

#[test]
fn pending_former_winner_marks_recommendation_after_falling_below_runner_up() {
    let fixture = synchronized_fixture_with(
        &[(1, "priority:p1"), (2, "priority:p3"), (3, "priority:p3")],
        1,
    );
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 1, "priority:p1", 1, 1);

    let queued = update_command(&fixture.state, &unavailable.url(), 1, "p4")
        .output()
        .expect("queue former-winner demotion");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("pending update JSON");
    let operation_id = queued["operation"]["id"]
        .as_str()
        .expect("operation ID")
        .to_owned();

    let ranked = next_command(&fixture.state, &unavailable.url())
        .output()
        .expect("rank after former winner falls below runner-up");
    assert_success(&ranked);
    let ranked: Value = serde_json::from_slice(&ranked.stdout).expect("pending next JSON");
    assert_eq!(ranked["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(ranked["comparison_to_runner_up"]["runner_up"]["number"], 3);
    assert_eq!(ranked["recommendation"]["first_issue"]["pending"], false);
    assert_eq!(ranked["recommendation"]["pending"], true);
    assert_eq!(
        ranked["recommendation"]["operation_ids"],
        json!([operation_id])
    );
    assert_eq!(ranked["recommendation"]["reasons"], json!([]));
    assert_eq!(
        ranked["comparison_to_runner_up"]["operation_ids"],
        ranked["recommendation"]["operation_ids"]
    );
    assert_eq!(ranked["comparison_to_runner_up"]["pending"], true);

    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

#[test]
fn concurrent_offline_updates_are_serialized_without_losing_intent() {
    let fixture = synchronized_fixture();
    let mut unavailable = Server::new();
    let unavailable_mocks = mock_unavailable(&mut unavailable, 1, "priority:p1", 2, 0);

    let mut first = update_command(&fixture.state, &unavailable.url(), 1, "p3");
    let mut second = update_command(&fixture.state, &unavailable.url(), 1, "p4");
    first.stdout(Stdio::piped()).stderr(Stdio::piped());
    second.stdout(Stdio::piped()).stderr(Stdio::piped());
    let first = first.spawn().expect("start first offline update");
    let second = second.spawn().expect("start second offline update");
    let first = first
        .wait_with_output()
        .expect("finish first offline update");
    let second = second
        .wait_with_output()
        .expect("finish second offline update");
    assert_success(&first);
    assert_success(&second);

    let outbox: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .state
                .path()
                .join("repositories/acme/offline/outbox.json"),
        )
        .expect("durable concurrent outbox"),
    )
    .expect("outbox JSON");
    let operations = outbox["operations"].as_array().expect("operations");
    assert_eq!(operations.len(), 2);
    assert_ne!(operations[0]["id"], operations[1]["id"]);
    assert_eq!(operations[1]["base"], operations[0]["desired"]);
    let desired: std::collections::BTreeSet<_> = operations
        .iter()
        .map(|operation| {
            operation["desired"]["value"]
                .as_str()
                .expect("desired value")
        })
        .collect();
    assert_eq!(desired, std::collections::BTreeSet::from(["p3", "p4"]));

    fixture.assert_replica_unchanged();
    unavailable_mocks.assert();
}

struct SynchronizedFixture {
    state: TempDir,
    replica_path: std::path::PathBuf,
    replica_bytes: Vec<u8>,
    replica_json: Value,
}

impl SynchronizedFixture {
    fn assert_replica_unchanged(&self) {
        assert_eq!(
            fs::read(&self.replica_path).expect("unchanged Local replica"),
            self.replica_bytes
        );
    }
}

fn synchronized_fixture() -> SynchronizedFixture {
    synchronized_fixture_with(&[(1, "priority:p1"), (2, "priority:p0")], 2)
}

fn synchronized_fixture_with(
    priorities: &[(u64, &str)],
    expected_recommendation: u64,
) -> SynchronizedFixture {
    let state = TempDir::new().expect("temporary state directory");
    let mut online = Server::new();
    let synchronized = mock_repository(&mut online, priorities);
    let baseline = next_command(&state, &online.url())
        .output()
        .expect("synchronize and rank baseline");
    assert_success(&baseline);
    let baseline: Value = serde_json::from_slice(&baseline.stdout).expect("baseline next JSON");
    assert_eq!(
        baseline["recommendation"]["first_issue"]["number"],
        expected_recommendation
    );
    assert_eq!(baseline["pending"], false);
    synchronized.assert();

    let replica_path = state.path().join("repositories/acme/offline/replica.json");
    let replica_bytes = fs::read(&replica_path).expect("synchronized Local replica");
    let replica_json = serde_json::from_slice(&replica_bytes).expect("Local replica JSON");
    SynchronizedFixture {
        state,
        replica_path,
        replica_bytes,
        replica_json,
    }
}

struct OfflineMocks {
    failed_issue_reads: Mock,
    failed_pulls: Mock,
    forbidden_add: Mock,
    forbidden_remove: Mock,
}

impl OfflineMocks {
    fn assert(self) {
        self.failed_issue_reads.assert();
        self.failed_pulls.assert();
        self.forbidden_add.assert();
        self.forbidden_remove.assert();
    }
}

fn mock_unavailable(
    github: &mut Server,
    issue_number: u64,
    current_priority: &str,
    issue_reads: usize,
    pulls: usize,
) -> OfflineMocks {
    let failed_issue_reads = github
        .mock(
            "GET",
            format!("/repos/acme/offline/issues/{issue_number}").as_str(),
        )
        .with_status(503)
        .expect(issue_reads)
        .create();
    let failed_pulls = github
        .mock("GET", "/repos/acme/offline/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(503)
        .expect(pulls)
        .create();
    let forbidden_add = github
        .mock(
            "POST",
            format!("/repos/acme/offline/issues/{issue_number}/labels").as_str(),
        )
        .expect(0)
        .create();
    let forbidden_remove = github
        .mock(
            "DELETE",
            format!("/repos/acme/offline/issues/{issue_number}/labels/{current_priority}").as_str(),
        )
        .expect(0)
        .create();
    OfflineMocks {
        failed_issue_reads,
        failed_pulls,
        forbidden_add,
        forbidden_remove,
    }
}

struct RepositoryMocks {
    events: Mock,
    labels: Mock,
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
}

impl RepositoryMocks {
    fn assert(self) {
        self.events.assert();
        self.labels.assert();
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
    }
}

fn mock_repository(github: &mut Server, priorities: &[(u64, &str)]) -> RepositoryMocks {
    let events = github
        .mock("GET", "/repos/acme/offline/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let labels = github
        .mock("GET", "/repos/acme/offline/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(canonical_labels().to_string())
        .create();
    let issues = github
        .mock("GET", "/repos/acme/offline/issues")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            Value::Array(
                priorities
                    .iter()
                    .map(|(number, priority)| issue(*number, &[*priority]))
                    .collect(),
            )
            .to_string(),
        )
        .create();
    let comments = github
        .mock("GET", "/repos/acme/offline/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependencies = priorities
        .iter()
        .map(|(number, _)| {
            github
                .mock(
                    "GET",
                    format!("/repos/acme/offline/issues/{number}/dependencies/blocked_by").as_str(),
                )
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body("[]")
                .create()
        })
        .collect();
    RepositoryMocks {
        events,
        labels,
        issues,
        comments,
        dependencies,
    }
}

fn next_command(state: &TempDir, api_url: &str) -> Command {
    command(
        state.path(),
        api_url,
        &["next", "--repo", "acme/offline", "--json"],
    )
}

fn ready_command(state: &TempDir, api_url: &str) -> Command {
    command(
        state.path(),
        api_url,
        &["ready", "--repo", "acme/offline", "--json"],
    )
}

fn update_command(state: &TempDir, api_url: &str, issue_number: u64, priority: &str) -> Command {
    command(
        state.path(),
        api_url,
        &[
            "update",
            &format!("acme/offline#{issue_number}"),
            "--priority",
            priority,
            "--json",
        ],
    )
}

fn command(state: &Path, api_url: &str, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command
        .args(args)
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state)
        .env("PATH", "");
    command
}

fn issue(number: u64, priority_labels: &[&str]) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": "open",
        "state_reason": null,
        "html_url": format!("https://github.com/acme/offline/issues/{number}"),
        "user": null,
        "assignees": [],
        "labels": priority_labels
            .iter()
            .enumerate()
            .map(|(index, name)| label(number * 10 + index as u64, name))
            .collect::<Vec<_>>(),
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": null
    })
}

fn canonical_labels() -> Value {
    json!([
        label(1, "priority:p0"),
        label(2, "priority:p1"),
        label(3, "priority:p2"),
        label(4, "priority:p3"),
        label(5, "priority:p4")
    ])
}

fn label(id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("L_{id}"),
        "name": name,
        "color": "123456",
        "description": null
    })
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
