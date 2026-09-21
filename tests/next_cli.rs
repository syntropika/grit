use std::process::Command;

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn next_evaluates_the_complete_frontier_with_and_unlocks_and_deduplication() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p3"], &[]),
        issue(2, "open", &["priority:p1"], &[]),
        issue(3, "open", &[], &[]),
        issue(4, "open", &["priority:p4"], &[]),
        issue(5, "open", &["priority:p1", "priority:p4"], &[]),
        issue(10, "open", &["priority:p1"], &[]),
        issue(11, "open", &["priority:p4"], &[]),
        issue(12, "open", &["priority:p3"], &[]),
        issue(13, "open", &["priority:p1"], &["alice"]),
        issue(14, "closed", &["priority:p0"], &[]),
        issue(15, "open", &["priority:p0"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![]),
        (4, vec![]),
        (5, vec![]),
        (
            10,
            vec![internal_blocker(1, "open"), internal_blocker(2, "open")],
        ),
        (
            11,
            vec![internal_blocker(1, "open"), internal_blocker(1, "open")],
        ),
        (12, vec![internal_blocker(1, "open")]),
        (13, vec![internal_blocker(1, "open")]),
        (14, vec![internal_blocker(2, "open")]),
        (
            15,
            vec![internal_blocker(11, "open"), internal_blocker(12, "open")],
        ),
    ];
    let mocks = mock_repository(&mut github, "acme/next", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/next", true)
        .output()
        .expect("run grit next");
    assert_success(&output);
    let next: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(next["schema_version"], "grit.next/v1");
    assert_eq!(next["policy_version"], "next/v1");
    assert_eq!(next["parameters"]["horizon"], 1);
    assert_eq!(next["mode"], "normal");
    assert_eq!(next["search_complete"], true);
    assert_eq!(next["truncated_by"], json!([]));
    assert_eq!(next["global_optimum_claimed"], true);
    assert_eq!(next["runner_up_scope"], "global");
    assert_ne!(next["replica_snapshot_hash"], Value::Null);
    assert_ne!(next["input_hash"], next["replica_snapshot_hash"]);
    assert_eq!(next["metrics"]["unlock_profile"]["state"], "available");
    assert_eq!(next["metrics"]["pagerank"]["state"], "available");

    let recommendation = &next["recommendation"];
    assert_eq!(recommendation["first_issue"]["number"], 1);
    assert_eq!(recommendation["rollout"]["steps"][0]["mode"], "normal");
    assert_eq!(recommendation["outcome"]["unlock_profile"]["count"], 3);
    assert_eq!(
        recommendation["outcome"]["unlock_profile"]["priority_profile"],
        json!({"p1": 1, "neutral": 0, "p3": 1, "p4": 1})
    );
    assert_eq!(
        recommendation["outcome"]["unlocks"]
            .as_array()
            .expect("unlocks")
            .iter()
            .map(|unlock| unlock["issue"]["number"].as_u64().expect("Issue number"))
            .collect::<Vec<_>>(),
        vec![11, 12, 13]
    );
    assert_eq!(
        recommendation["outcome"]["unlock_availability"],
        json!({"available": 2, "assigned": 1})
    );
    assert_eq!(
        next["alternatives"]
            .as_array()
            .expect("alternatives")
            .iter()
            .map(|alternative| alternative["first_issue"]["number"]
                .as_u64()
                .expect("Issue number"))
            .collect::<Vec<_>>(),
        vec![2, 3, 5, 4]
    );
    assert_eq!(
        next["comparison_to_runner_up"]["reason_code"],
        "unlocks_more_work"
    );
    assert_eq!(next["comparison_to_runner_up"]["runner_up"]["number"], 2);
    assert_eq!(next["close_call"], false);
    assert_eq!(next["warnings"][0]["code"], "priority_conflict");
    assert_eq!(next["warnings"][0]["issue_number"], 5);
    mocks.assert();

    let ready = ready_command(&state, &github.url(), "acme/next")
        .output()
        .expect("offline grit ready");
    assert_success(&ready);
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("ready JSON");
    let ready_numbers = ready["issues"]
        .as_array()
        .expect("ready Issues")
        .iter()
        .map(|issue| issue["number"].as_u64().expect("Issue number"))
        .collect::<Vec<_>>();
    let mut ranked_numbers = vec![
        next["recommendation"]["first_issue"]["number"]
            .as_u64()
            .expect("recommendation number"),
    ];
    ranked_numbers.extend(
        next["alternatives"]
            .as_array()
            .expect("alternatives")
            .iter()
            .map(|alternative| {
                alternative["first_issue"]["number"]
                    .as_u64()
                    .expect("alternative number")
            }),
    );
    ranked_numbers.sort_unstable();
    assert_eq!(ranked_numbers, ready_numbers);
}

#[test]
fn executable_p0_gate_prefers_the_p0_that_unlocks_more_critical_work() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p0"], &[]),
        issue(2, "open", &["priority:p0"], &[]),
        issue(3, "open", &["priority:p1"], &[]),
        issue(10, "open", &["priority:p4"], &[]),
        issue(11, "open", &["priority:p0"], &[]),
        issue(20, "open", &["priority:p1"], &[]),
        issue(21, "open", &["priority:p1"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![]),
        (10, vec![internal_blocker(1, "open")]),
        (11, vec![internal_blocker(2, "open")]),
        (20, vec![internal_blocker(3, "open")]),
        (21, vec![internal_blocker(3, "open")]),
    ];
    let mocks = mock_repository(&mut github, "acme/p0-ready", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/p0-ready", true)
        .output()
        .expect("run P0 next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(output["mode"], "p0_ready");
    assert_eq!(output["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(
        output["recommendation"]["outcome"]["unlock_profile"]["p0_curve"],
        json!([1])
    );
    assert_eq!(
        output["alternatives"]
            .as_array()
            .expect("alternatives")
            .len(),
        1
    );
    assert_eq!(output["alternatives"][0]["first_issue"]["number"], 1);
    assert!(reason_codes(&output).contains(&"ready_p0"));
    assert!(reason_codes(&output).contains(&"unlocks_more_p0"));
    mocks.assert();
}

#[test]
fn one_step_p0_route_beats_a_larger_noncritical_fanout() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mut issues = vec![
        issue(1, "open", &["priority:p4"], &[]),
        issue(2, "open", &["priority:p1"], &[]),
        issue(10, "open", &["priority:p0"], &[]),
    ];
    let mut dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (10, vec![internal_blocker(1, "open")]),
    ];
    for number in 20..26 {
        issues.push(issue(number, "open", &["priority:p1"], &[]));
        dependencies.push((number, vec![internal_blocker(2, "open")]));
    }
    let mocks = mock_repository(&mut github, "acme/p0-route", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/p0-route", true)
        .output()
        .expect("run routed P0 next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(output["mode"], "p0_route");
    assert_eq!(output["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(output["alternatives"], json!([]));
    assert!(reason_codes(&output).contains(&"shortest_p0_route"));
    mocks.assert();
}

#[test]
fn pagerank_then_stable_node_key_break_only_structural_ties() {
    let state = TempDir::new().expect("temporary state directory");
    let mut pagerank_github = Server::new();
    let issues = vec![
        issue(1, "open", &[], &[]),
        issue(2, "open", &[], &[]),
        issue(3, "open", &[], &[]),
        issue(10, "open", &[], &[]),
        issue(11, "open", &[], &[]),
        issue(12, "open", &[], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![external_blocker("partners/api", 90, "unknown")]),
        (
            10,
            vec![internal_blocker(1, "open"), internal_blocker(3, "open")],
        ),
        (
            11,
            vec![internal_blocker(1, "open"), internal_blocker(3, "open")],
        ),
        (
            12,
            vec![internal_blocker(2, "open"), internal_blocker(3, "open")],
        ),
    ];
    let mocks = mock_repository(&mut pagerank_github, "acme/pagerank", issues, dependencies);
    let pagerank = next_command(&state, &pagerank_github.url(), "acme/pagerank", true)
        .output()
        .expect("run PageRank next");
    assert_success(&pagerank);
    let pagerank: Value = serde_json::from_slice(&pagerank.stdout).expect("next JSON");
    assert_eq!(pagerank["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(
        pagerank["comparison_to_runner_up"]["reason_code"],
        "pagerank_tiebreak"
    );
    assert_eq!(pagerank["close_call"], true);
    assert!(
        pagerank["recommendation"]["pagerank_bucket"]
            .as_u64()
            .expect("winner bucket")
            > pagerank["alternatives"][0]["pagerank_bucket"]
                .as_u64()
                .expect("runner-up bucket")
    );
    mocks.assert();

    let mut stable_github = Server::new();
    let stable_state = TempDir::new().expect("stable-key state directory");
    let stable_mocks = mock_repository(
        &mut stable_github,
        "acme/stable",
        vec![issue(2, "open", &[], &[]), issue(1, "open", &[], &[])],
        vec![(2, vec![]), (1, vec![])],
    );
    let stable = next_command(&stable_state, &stable_github.url(), "acme/stable", true)
        .output()
        .expect("run stable next");
    assert_success(&stable);
    let stable: Value = serde_json::from_slice(&stable.stdout).expect("next JSON");
    assert_eq!(stable["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(
        stable["comparison_to_runner_up"]["reason_code"],
        "deterministic_tiebreak"
    );
    assert_eq!(stable["close_call"], true);
    stable_mocks.assert();
}

#[test]
fn bounded_output_keeps_a_late_winner_and_the_global_runner_up() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = (1..=13)
        .map(|number| {
            let priority = if number == 13 {
                "priority:p1"
            } else {
                "priority:p4"
            };
            issue(number, "open", &[priority], &[])
        })
        .collect();
    let dependencies = (1..=13).map(|number| (number, vec![])).collect();
    let mocks = mock_repository(&mut github, "acme/bounded", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/bounded", true)
        .output()
        .expect("run bounded next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["summary"]["candidate_count"], 13);
    assert_eq!(output["recommendation"]["first_issue"]["number"], 13);
    assert_eq!(output["comparison_to_runner_up"]["runner_up"]["number"], 1);
    assert_eq!(
        output["comparison_to_runner_up"]["reason_code"],
        "declared_priority_tiebreak"
    );
    assert_eq!(
        output["alternatives"]
            .as_array()
            .expect("bounded alternatives")
            .iter()
            .map(|candidate| candidate["first_issue"]["number"]
                .as_u64()
                .expect("Issue number"))
            .collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
    mocks.assert();
}

#[test]
fn no_candidate_succeeds_with_categories_and_deterministic_offline_json() {
    let mut github = Server::new();
    let api_url = github.url();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &[], &["alice"]),
        issue(2, "open", &[], &[]),
        issue(3, "open", &[], &[]),
        issue(4, "open", &[], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![internal_blocker(3, "open")]),
        (3, vec![internal_blocker(2, "open")]),
        (4, vec![external_blocker("private/unknown", 99, "unknown")]),
    ];
    let mocks = mock_repository(&mut github, "acme/none", issues, dependencies);
    let online = next_command(&state, &api_url, "acme/none", true)
        .output()
        .expect("seed Local replica");
    assert_success(&online);
    mocks.assert();
    drop(github);

    let first = next_command(&state, &api_url, "acme/none", false)
        .output()
        .expect("first offline next");
    let second = next_command(&state, &api_url, "acme/none", false)
        .output()
        .expect("second offline next");
    assert_success(&first);
    assert_success(&second);
    assert_eq!(first.stdout, second.stdout);
    let output: Value = serde_json::from_slice(&first.stdout).expect("next JSON");
    assert_eq!(output["source"], "local_fallback");
    assert_eq!(output["recommendation"], Value::Null);
    assert_eq!(output["alternatives"], json!([]));
    assert_eq!(output["summary"]["blocked_count"], 3);
    assert_eq!(output["summary"]["assigned_ready_count"], 1);
    assert_eq!(output["summary"]["cyclic_issue_count"], 2);
    assert_eq!(output["summary"]["unknown_blocker_count"], 1);
    assert_eq!(output["warnings"][0]["code"], "offline_fallback");

    let assigned = next_command(&state, &api_url, "acme/none", false)
        .arg("--assignee")
        .arg("alice")
        .output()
        .expect("offline assignee next");
    assert_success(&assigned);
    let assigned: Value = serde_json::from_slice(&assigned.stdout).expect("assignee next JSON");
    assert_eq!(
        assigned["execution_scope"],
        json!({"mode": "assignee", "assignee": "alice"})
    );
    assert_eq!(assigned["recommendation"]["first_issue"]["number"], 1);
    assert_ne!(assigned["input_hash"], output["input_hash"]);
}

#[test]
fn empty_graph_omits_pagerank_globally_and_horizon_two_is_rejected() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mocks = mock_repository(&mut github, "acme/empty", vec![], vec![]);
    let output = next_command(&state, &github.url(), "acme/empty", true)
        .output()
        .expect("run empty next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(output["metrics"]["pagerank"]["state"], "omitted");
    assert_eq!(output["recommendation"], Value::Null);
    assert_eq!(output["alternatives"], json!([]));
    mocks.assert();

    let unsupported = Command::new(env!("CARGO_BIN_EXE_grit"))
        .args(["next", "--repo", "acme/empty", "--horizon", "2", "--json"])
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "")
        .output()
        .expect("unsupported horizon");
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("horizon 1"));
}

fn next_command(state: &TempDir, api_url: &str, repository: &str, online: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["next", "--repo", repository, "--horizon", "1", "--json"]);
    command
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    if online {
        command.env("GH_TOKEN", "automation-token");
    } else {
        command.env_remove("GH_TOKEN");
    }
    command
}

fn ready_command(state: &TempDir, api_url: &str, repository: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["ready", "--repo", repository, "--json"]);
    command
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env_remove("GH_TOKEN")
        .env("PATH", "");
    command
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

fn mock_repository(
    github: &mut Server,
    repository: &str,
    issues: Vec<Value>,
    dependencies: Vec<(u64, Vec<Value>)>,
) -> RepositoryMocks {
    let labels_path = format!("/repos/{repository}/labels");
    let events_path = format!("/repos/{repository}/issues/events");
    let events = github
        .mock("GET", events_path.as_str())
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let labels = github
        .mock("GET", labels_path.as_str())
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(canonical_labels().to_string())
        .create();
    let issues_path = format!("/repos/{repository}/issues");
    let issue_response = Value::Array(issues);
    let issues = github
        .mock("GET", issues_path.as_str())
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(issue_response.to_string())
        .create();
    let comments_path = format!("/repos/{repository}/issues/comments");
    let comments = github
        .mock("GET", comments_path.as_str())
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependencies = dependencies
        .into_iter()
        .map(|(number, blockers)| {
            let blockers = blockers
                .into_iter()
                .map(|mut blocker| {
                    if blocker["repository_url"] == "https://api.github.com/repos/acme/placeholder"
                    {
                        blocker["repository_url"] =
                            Value::String(format!("https://api.github.com/repos/{repository}"));
                    }
                    blocker
                })
                .collect();
            let path = format!("/repos/{repository}/issues/{number}/dependencies/blocked_by");
            github
                .mock("GET", path.as_str())
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(Value::Array(blockers).to_string())
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

fn issue(number: u64, state: &str, priority_labels: &[&str], assignees: &[&str]) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://github.com/acme/repo/issues/{number}"),
        "user": null,
        "assignees": assignees
            .iter()
            .enumerate()
            .map(|(index, login)| actor(number * 1000 + index as u64, login))
            .collect::<Vec<_>>(),
        "labels": priority_labels
            .iter()
            .enumerate()
            .map(|(index, name)| label(number * 10 + index as u64, name))
            .collect::<Vec<_>>(),
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": if state == "closed" { Some("2026-08-02T00:00:00Z") } else { None }
    })
}

fn internal_blocker(number: u64, state: &str) -> Value {
    blocker("acme/placeholder", number, state)
}

fn external_blocker(repository: &str, number: u64, state: &str) -> Value {
    blocker(repository, number, state)
}

fn blocker(repository: &str, number: u64, state: &str) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "repository_url": format!("https://api.github.com/repos/{repository}"),
        "number": number,
        "state": state
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

fn actor(id: u64, login: &str) -> Value {
    json!({"id": id, "node_id": format!("U_{id}"), "login": login})
}

fn reason_codes(output: &Value) -> Vec<&str> {
    output["recommendation"]["reasons"]
        .as_array()
        .expect("recommendation reasons")
        .iter()
        .map(|reason| reason["code"].as_str().expect("reason code"))
        .collect()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
