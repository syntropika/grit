use std::{fs, process::Command};

use mockito::{Matcher, Mock, Server};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[test]
fn init_creates_only_missing_priority_labels_and_then_converges() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let initial_labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!([
                label(1, "Priority:P0", "abcdef", "A custom description"),
                label(2, "area:core", "123456", "Unrelated")
            ])
            .to_string(),
        )
        .create();
    let creations: Vec<_> = [
        ("p1", "d93f0b", "High priority"),
        ("p2", "fbca04", "Normal priority"),
        ("p3", "0e8a16", "Low priority"),
        ("p4", "6a737d", "Lowest priority"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (priority, color, description))| {
        mock_label_creation(&mut github, priority, color, description, index as u64 + 10)
    })
    .collect();

    let first = init_command(&state, &github.url())
        .output()
        .expect("first init");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("init JSON");
    assert_eq!(first["schema_version"], "grit.init/v1");
    assert_eq!(first["command"], "init");
    assert_eq!(first["repository"], "acme/widgets");
    assert_eq!(
        first["created_labels"],
        json!(["priority:p1", "priority:p2", "priority:p3", "priority:p4"])
    );
    assert_eq!(first["already_present"], json!(["priority:p0"]));
    initial_labels.assert();
    for creation in &creations {
        creation.assert();
    }
    drop(initial_labels);
    drop(creations);

    let complete_labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::to_string(
                &(0_u64..=4)
                    .map(|priority| {
                        label(
                            priority + 20,
                            &format!("priority:p{priority}"),
                            "fedcba",
                            "Existing style remains untouched",
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .expect("complete labels"),
        )
        .create();

    let second = init_command(&state, &github.url())
        .output()
        .expect("idempotent init");
    assert_success(&second);
    let second: Value = serde_json::from_slice(&second.stdout).expect("init JSON");
    assert_eq!(second["created_labels"], json!([]));
    assert_eq!(
        second["already_present"],
        json!([
            "priority:p0",
            "priority:p1",
            "priority:p2",
            "priority:p3",
            "priority:p4"
        ])
    );
    complete_labels.assert();
}

#[test]
fn init_accepts_a_concurrent_canonical_label_creation() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!([
                label(20, "priority:p0", "b60205", "Critical priority"),
                label(22, "priority:p2", "fbca04", "Normal priority"),
                label(23, "priority:p3", "0e8a16", "Low priority"),
                label(24, "priority:p4", "6a737d", "Lowest priority")
            ])
            .to_string(),
        )
        .create();
    let concurrent_creation = github
        .mock("POST", "/repos/acme/widgets/labels")
        .match_body(Matcher::Json(json!({
            "name": "priority:p1",
            "color": "d93f0b",
            "description": "High priority"
        })))
        .with_status(422)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "message": "Validation Failed",
                "errors": [{
                    "resource": "Label",
                    "field": "name",
                    "code": "already_exists"
                }]
            })
            .to_string(),
        )
        .create();

    let output = init_command(&state, &github.url())
        .output()
        .expect("concurrent init");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("init JSON");
    assert_eq!(output["created_labels"], json!([]));
    assert_eq!(
        output["already_present"],
        json!([
            "priority:p0",
            "priority:p1",
            "priority:p2",
            "priority:p3",
            "priority:p4"
        ])
    );
    labels.assert();
    concurrent_creation.assert();
}

#[test]
fn ready_reports_declared_unspecified_and_conflicting_priority_without_mutation() {
    let mut github = Server::new();
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(2)
        .create();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, vec![label(20, "priority:p2", "fbca04", "Normal")]),
        issue_with_fields(2, Vec::new()),
        issue(
            3,
            vec![
                label(30, "priority:p0", "b60205", "Critical"),
                label(34, "priority:p4", "6a737d", "Lowest"),
            ],
        ),
    ];
    let issue_mock = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&issues).expect("Issues"))
        .expect(2)
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(2)
        .create();
    let dependencies: Vec<_> = (1_u64..=3)
        .map(|number| {
            github
                .mock(
                    "GET",
                    format!("/repos/acme/widgets/issues/{number}/dependencies/blocked_by").as_str(),
                )
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body("[]")
                .expect(2)
                .create()
        })
        .collect();
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!([
                label(20, "priority:p0", "b60205", "Critical"),
                label(22, "priority:p2", "fbca04", "Normal"),
                label(24, "priority:p4", "6a737d", "Lowest")
            ])
            .to_string(),
        )
        .expect(2)
        .create();

    let output = ready_command(&state, &github.url())
        .output()
        .expect("priority-aware ready");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("ready JSON");
    assert_eq!(output["issues"].as_array().expect("Issues").len(), 3);
    assert_eq!(
        output["issues"][0]["priority"],
        json!({"state": "declared", "comparison": "neutral", "value": "p2"})
    );
    assert_eq!(
        output["issues"][1]["priority"],
        json!({"state": "unspecified", "comparison": "neutral"})
    );
    assert_eq!(
        output["issues"][2]["priority"],
        json!({
            "state": "conflict",
            "comparison": "neutral",
            "labels": ["priority:p0", "priority:p4"]
        })
    );
    assert_eq!(output["summary"]["ready_count"], 3);
    assert_eq!(output["summary"]["executable_count"], 3);

    let warnings = output["warnings"].as_array().expect("warnings");
    assert!(warnings.iter().any(|warning| {
        warning["code"] == "missing_priority_labels"
            && warning["labels"] == json!(["priority:p1", "priority:p3"])
    }));

    let human = ready_command_human(&state, &github.url())
        .output()
        .expect("human priority-aware ready");
    assert_success(&human);
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(stderr.contains("priority:p1, priority:p3"));
    assert!(stderr.contains("priority:p0, priority:p4"));
    assert!(warnings.iter().any(|warning| {
        warning["code"] == "priority_conflict"
            && warning["issue_number"] == 3
            && warning["labels"] == json!(["priority:p0", "priority:p4"])
    }));

    events.assert();
    issue_mock.assert();
    comments.assert();
    for dependency in dependencies {
        dependency.assert();
    }
    labels.assert();
}

#[test]
fn a_legacy_replica_does_not_claim_its_unknown_label_catalog_is_empty() {
    let state = TempDir::new().expect("temporary state directory");
    let replica_dir = state.path().join("repositories/acme/widgets");
    fs::create_dir_all(&replica_dir).expect("replica directory");
    let issues: Vec<Value> = Vec::new();
    let dependencies: Vec<Value> = Vec::new();
    let hash_input = LegacyHashInput {
        schema_version: "grit.local-replica/v1",
        repository: "acme/widgets",
        issues: &issues,
        dependencies: &dependencies,
    };
    let input_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&hash_input).expect("legacy hash input"),
    ));
    let replica = json!({
        "schema_version": "grit.local-replica/v1",
        "repository": "acme/widgets",
        "synced_at": "2026-08-01T00:00:00Z",
        "input_hash": input_hash,
        "issues": [],
        "dependencies": []
    });
    fs::write(
        replica_dir.join("replica.json"),
        serde_json::to_vec(&replica).expect("legacy replica JSON"),
    )
    .expect("legacy replica");

    let output = ready_command(&state, "http://127.0.0.1:1")
        .env_remove("GH_TOKEN")
        .output()
        .expect("offline ready from legacy replica");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("ready JSON");
    assert_eq!(output["source"], "local_fallback");
    assert_eq!(output["warnings"].as_array().expect("warnings").len(), 1);
    assert_eq!(output["warnings"][0]["code"], "offline_fallback");
}

fn mock_label_creation(
    github: &mut Server,
    priority: &str,
    color: &str,
    description: &str,
    id: u64,
) -> Mock {
    let name = format!("priority:{priority}");
    github
        .mock("POST", "/repos/acme/widgets/labels")
        .match_body(Matcher::Json(json!({
            "name": name,
            "color": color,
            "description": description
        })))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(label(id, &name, color, description).to_string())
        .create()
}

#[derive(Serialize)]
struct LegacyHashInput<'a> {
    schema_version: &'a str,
    repository: &'a str,
    issues: &'a [Value],
    dependencies: &'a [Value],
}

fn label(id: u64, name: &str, color: &str, description: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("L_{id}"),
        "name": name,
        "color": color,
        "description": description
    })
}

fn issue(number: u64, labels: Vec<Value>) -> Value {
    let mut issue = issue_with_fields(number, labels);
    issue
        .as_object_mut()
        .expect("Issue object")
        .remove("issue_field_values");
    issue
}

fn issue_with_fields(number: u64, labels: Vec<Value>) -> Value {
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
        "labels": labels,
        "issue_field_values": [{"name": "Priority", "value": "Urgent"}],
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": null
    })
}

fn init_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["init", "--repo", "acme/widgets", "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_NO_KEYRING", "1")
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
        .env("GRIT_NO_KEYRING", "1")
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn ready_command_human(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["ready", "--repo", "acme/widgets"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_NO_KEYRING", "1")
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
