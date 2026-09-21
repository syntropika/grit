use std::process::Command;

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn triage_reports_actionable_operational_problems_and_ignores_closed_history() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mocks = mock_repository(&mut github);

    let output = triage_command(&state, &github.url(), None, true, true)
        .output()
        .expect("live triage");
    assert_success(&output);
    let live: Value = serde_json::from_slice(&output.stdout).expect("triage JSON");
    assert_eq!(live["schema_version"], "grit.triage/v1");
    assert_eq!(live["command"], "triage");
    assert_eq!(live["repository"], "acme/widgets");
    assert_eq!(live["source"], "live");
    assert_eq!(live["execution_scope"], json!({"mode": "available"}));
    assert_eq!(
        diagnostic_codes(&live),
        vec![
            "blocked_p0",
            "external_blocker_open",
            "external_blocker_unknown",
            "dependency_cycle",
            "assigned_ready",
            "priority_conflict"
        ]
    );
    assert_eq!(live["summary"]["diagnostic_count"], 6);

    let blocked_p0 = &live["diagnostics"][0];
    assert_eq!(blocked_p0["subjects"][0]["key"], "acme/widgets#1");
    assert_eq!(blocked_p0["subjects"][0]["readiness"], "blocked");
    assert_eq!(blocked_p0["subjects"][0]["availability"], "available");
    assert_eq!(blocked_p0["subjects"][0]["in_execution_scope"], false);
    assert_eq!(blocked_p0["blockers"][0]["key"], "acme/widgets#2");

    assert_eq!(
        live["diagnostics"][1]["blockers"],
        json!([{
            "key": "partners/api#90",
            "scope": "external",
            "state": "open"
        }])
    );
    assert_eq!(live["diagnostics"][2]["blockers"][0]["state"], "unknown");
    assert_eq!(
        live["diagnostics"][3]["subjects"]
            .as_array()
            .expect("cycle subjects")
            .iter()
            .map(|subject| subject["key"].as_str().expect("key"))
            .collect::<Vec<_>>(),
        vec!["acme/widgets#5", "acme/widgets#6"]
    );
    let assigned = &live["diagnostics"][4]["subjects"][0];
    assert_eq!(assigned["key"], "acme/widgets#7");
    assert_eq!(assigned["readiness"], "ready");
    assert_eq!(assigned["availability"], "assigned");
    assert_eq!(assigned["in_execution_scope"], false);
    let conflict = &live["diagnostics"][5];
    assert_eq!(conflict["subjects"][0]["key"], "acme/widgets#8");
    assert_eq!(conflict["subjects"][0]["readiness"], "ready");
    assert_eq!(conflict["subjects"][0]["availability"], "available");
    assert_eq!(conflict["subjects"][0]["in_execution_scope"], true);
    assert_eq!(conflict["labels"], json!(["priority:p1", "priority:p3"]));
    let serialized = String::from_utf8(output.stdout).expect("UTF-8 JSON");
    assert!(!serialized.contains("acme/widgets#9"));
    assert!(!serialized.contains("acme/widgets#10"));
    assert!(!serialized.contains("acme/widgets#11"));
    mocks.assert();
}

#[test]
fn triage_keeps_availability_separate_from_an_offline_assignee_scope() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mocks = mock_repository(&mut github);
    let live = triage_command(&state, &github.url(), None, true, true)
        .output()
        .expect("live triage seed");
    assert_success(&live);
    mocks.assert();

    let assigned_scope = triage_command(&state, &github.url(), Some("alice"), true, false)
        .output()
        .expect("offline assignee triage");
    assert_success(&assigned_scope);
    let assigned_scope: Value =
        serde_json::from_slice(&assigned_scope.stdout).expect("triage JSON");
    assert_eq!(assigned_scope["source"], "local_fallback");
    assert_eq!(
        assigned_scope["execution_scope"],
        json!({"mode": "assignee", "assignee": "alice"})
    );
    assert_eq!(
        assigned_scope["diagnostics"][4]["subjects"][0]["availability"],
        "assigned"
    );
    assert_eq!(
        assigned_scope["diagnostics"][4]["subjects"][0]["in_execution_scope"],
        true
    );
    assert_eq!(
        assigned_scope["diagnostics"][5]["subjects"][0]["in_execution_scope"],
        false
    );
}

#[test]
fn human_triage_names_the_scope_and_annotates_each_operational_state() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mocks = mock_repository(&mut github);
    let human = triage_command(&state, &github.url(), Some("alice"), false, true)
        .output()
        .expect("human triage");
    assert_success(&human);
    let stdout = String::from_utf8(human.stdout).expect("human output");
    assert!(stdout.contains("scope assignee:alice"));
    assert!(stdout.contains("[blocked_p0] acme/widgets#1"));
    assert!(stdout.contains("[dependency_cycle] acme/widgets#5, acme/widgets#6"));
    assert!(stdout.contains("[priority_conflict] acme/widgets#8"));
    assert!(stdout.contains(
        "acme/widgets#7: readiness=ready, availability=assigned, in_execution_scope=true"
    ));
    assert!(stdout.contains(
        "acme/widgets#8: readiness=ready, availability=available, in_execution_scope=false"
    ));
    mocks.assert();
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

fn mock_repository(github: &mut Server) -> RepositoryMocks {
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!([
                label(20, "priority:p0"),
                label(21, "priority:p1"),
                label(22, "priority:p2"),
                label(23, "priority:p3"),
                label(24, "priority:p4")
            ])
            .to_string(),
        )
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
        .with_body(issue_inventory().to_string())
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependency_bodies = [
        (1, internal_blockers(&[(2, "open")])),
        (2, "[]".to_owned()),
        (3, external_blocker("partners/api", 90, "open")),
        (4, external_blocker("partners/data", 91, "mystery")),
        (5, internal_blockers(&[(6, "open")])),
        (6, internal_blockers(&[(5, "open")])),
        (7, "[]".to_owned()),
        (8, "[]".to_owned()),
        (9, external_blocker("private/legacy", 92, "mystery")),
        (10, internal_blockers(&[(11, "closed")])),
        (11, internal_blockers(&[(10, "closed")])),
    ];
    let dependencies = dependency_bodies
        .into_iter()
        .map(|(number, body)| {
            github
                .mock(
                    "GET",
                    format!("/repos/acme/widgets/issues/{number}/dependencies/blocked_by").as_str(),
                )
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(body)
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

fn issue_inventory() -> Value {
    json!([
        issue(1, "open", vec![label(20, "priority:p0")], vec![]),
        issue(2, "open", vec![], vec![]),
        issue(3, "open", vec![], vec![]),
        issue(4, "open", vec![], vec![]),
        issue(5, "open", vec![], vec![]),
        issue(6, "open", vec![], vec![]),
        issue(7, "open", vec![], vec![actor(70, "alice")]),
        issue(
            8,
            "open",
            vec![label(21, "priority:p1"), label(23, "priority:p3")],
            vec![]
        ),
        issue(
            9,
            "closed",
            vec![label(20, "priority:p0"), label(24, "priority:p4")],
            vec![actor(90, "historian")]
        ),
        issue(10, "closed", vec![], vec![]),
        issue(11, "closed", vec![], vec![])
    ])
}

fn triage_command(
    state: &TempDir,
    api_url: &str,
    assignee: Option<&str>,
    json: bool,
    online: bool,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["triage", "--repo", "acme/widgets"]);
    if let Some(assignee) = assignee {
        command.args(["--assignee", assignee]);
    }
    if json {
        command.arg("--json");
    }
    command
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_NO_KEYRING", "1")
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    if online {
        command.env("GH_TOKEN", "automation-token");
    } else {
        command.env_remove("GH_TOKEN");
    }
    command
}

fn issue(number: u64, state: &str, labels: Vec<Value>, assignees: Vec<Value>) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": null,
        "assignees": assignees,
        "labels": labels,
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": if state == "closed" { Some("2026-08-02T00:00:00Z") } else { None }
    })
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

fn internal_blockers(blockers: &[(u64, &str)]) -> String {
    Value::Array(
        blockers
            .iter()
            .map(|(number, state)| blocker("acme/widgets", *number, state))
            .collect(),
    )
    .to_string()
}

fn external_blocker(repository: &str, number: u64, state: &str) -> String {
    json!([blocker(repository, number, state)]).to_string()
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

fn diagnostic_codes(output: &Value) -> Vec<&str> {
    output["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().expect("reason code"))
        .collect()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
