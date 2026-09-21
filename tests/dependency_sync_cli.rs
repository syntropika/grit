use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

mod support;

#[test]
fn relationship_events_update_the_graph_without_issue_timestamp_changes() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "First");
    let two = issue(2, "Second");
    let initial = mock_full(
        &mut github,
        &[one.clone(), two.clone()],
        &[(1, "[]"), (2, "[]")],
        event_page(vec![ordinary_event(100, 1)]),
        1,
    );

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    let since = support::replica_since(&state, "acme/widgets");

    let issues = mock_issue_delta(&mut github, &since, vec![one, two]);
    let comments = mock_comment_delta(&mut github, &since, vec![comment(301, 3)]);
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(event_page(vec![
            dependency_event(102, "blocking_added", 1, "blocking", 3),
            dependency_event(101, "blocked_by_added", 3, "blocked_by", 1),
            ordinary_event(100, 1),
        ]))
        .create();
    let unknown_issue = github
        .mock("GET", "/repos/acme/widgets/issues/3")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(issue(3, "Discovered from the event").to_string())
        .create();
    let unknown_comments = github
        .mock("GET", "/repos/acme/widgets/issues/3/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&vec![comment(300, 3)]).expect("existing comments"))
        .create();
    let unknown_dependencies = github
        .mock(
            "GET",
            "/repos/acme/widgets/issues/3/dependencies/blocked_by",
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let count = support::mock_issue_count(&mut github, 3);

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("relationship-only sync"),
    );
    issues.assert();
    comments.assert();
    events.assert();
    unknown_issue.assert();
    unknown_comments.assert();
    unknown_dependencies.assert();
    count.assert();

    let replica = support::load_replica(&state, "acme/widgets");
    assert!(replica_issue(&replica, 3).is_some());
    let discovered_comments = replica_issue(&replica, 3).expect("discovered Issue")["comments"]
        .as_array()
        .expect("discovered comments");
    assert_eq!(discovered_comments.len(), 2);
    assert_eq!(discovered_comments[0]["id"], 300);
    assert_eq!(discovered_comments[1]["id"], 301);
    assert_eq!(
        replica["dependencies"]
            .as_array()
            .expect("Dependencies")
            .len(),
        1
    );
    assert_eq!(replica["dependencies"][0]["blocked"]["number"], 3);
    assert_eq!(replica["dependencies"][0]["blocker"]["number"], 1);
    assert_eq!(replica["sync"]["dependency_events"]["latest_event_id"], 102);
}

#[test]
fn a_missing_event_checkpoint_repairs_the_graph_with_full_reconciliation() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "First");
    let two = issue(2, "Second");
    let initial = mock_full(
        &mut github,
        &[one.clone(), two.clone()],
        &[(1, "[]"), (2, "[]")],
        event_page(vec![ordinary_event(100, 1)]),
        1,
    );
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    let since = support::replica_since(&state, "acme/widgets");

    let delta_issues = mock_issue_delta(&mut github, &since, vec![one.clone(), two.clone()]);
    let delta_comments = mock_comment_delta(&mut github, &since, Vec::new());
    let gap_events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .expect(2)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(event_page(vec![dependency_event(
            200,
            "blocked_by_added",
            2,
            "blocked_by",
            1,
        )]))
        .create();
    let blocker = blocker(1);
    let repaired = mock_full(
        &mut github,
        &[one, two],
        &[
            (1, "[]"),
            (2, &serde_json::to_string(&vec![blocker]).expect("blocker")),
        ],
        String::new(),
        0,
    );

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("gap repair sync"),
    );
    delta_issues.assert();
    delta_comments.assert();
    gap_events.assert();
    repaired.assert_without_events();

    let replica = support::load_replica(&state, "acme/widgets");
    assert_eq!(
        replica["dependencies"]
            .as_array()
            .expect("Dependencies")
            .len(),
        1
    );
    assert_eq!(replica["dependencies"][0]["blocked"]["number"], 2);
    assert_eq!(replica["dependencies"][0]["blocker"]["number"], 1);
    assert_eq!(replica["sync"]["dependency_events"]["latest_event_id"], 200);
}

#[test]
fn mirrored_removal_events_delete_one_canonical_dependency() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "First");
    let two = issue(2, "Second");
    let initial_blockers = serde_json::to_string(&vec![blocker(1)]).expect("blocker");
    let initial = mock_full(
        &mut github,
        &[one.clone(), two.clone()],
        &[(1, "[]"), (2, &initial_blockers)],
        event_page(vec![ordinary_event(100, 1)]),
        1,
    );
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    assert_eq!(
        support::load_replica(&state, "acme/widgets")["dependencies"]
            .as_array()
            .expect("Dependencies")
            .len(),
        1
    );
    let since = support::replica_since(&state, "acme/widgets");

    let issues = mock_issue_delta(&mut github, &since, vec![one, two]);
    let comments = mock_comment_delta(&mut github, &since, Vec::new());
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(event_page(vec![
            dependency_event(102, "blocking_removed", 1, "blocking", 2),
            dependency_event(101, "blocked_by_removed", 2, "blocked_by", 1),
            ordinary_event(100, 1),
        ]))
        .create();
    let count = support::mock_issue_count(&mut github, 2);

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("relationship removal sync"),
    );
    issues.assert();
    comments.assert();
    events.assert();
    count.assert();
    assert!(
        support::load_replica(&state, "acme/widgets")["dependencies"]
            .as_array()
            .expect("Dependencies")
            .is_empty()
    );
}

#[test]
fn opposite_same_second_events_replay_in_feed_chronology_not_event_id_order() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "First");
    let two = issue(2, "Second");
    let initial = mock_full(
        &mut github,
        &[one.clone(), two.clone()],
        &[(1, "[]"), (2, "[]")],
        event_page(vec![ordinary_event(100, 1)]),
        1,
    );
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    let since = support::replica_since(&state, "acme/widgets");

    let issues = mock_issue_delta(&mut github, &since, vec![one, two]);
    let comments = mock_comment_delta(&mut github, &since, Vec::new());
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(event_page(vec![
            dependency_event(7, "blocked_by_removed", 2, "blocked_by", 1),
            dependency_event(900, "blocked_by_added", 2, "blocked_by", 1),
            ordinary_event(100, 1),
        ]))
        .create();
    let count = support::mock_issue_count(&mut github, 2);

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("same-second relationship sync"),
    );
    issues.assert();
    comments.assert();
    events.assert();
    count.assert();
    let replica = support::load_replica(&state, "acme/widgets");
    assert!(
        replica["dependencies"]
            .as_array()
            .expect("Dependencies")
            .is_empty()
    );
    assert_eq!(replica["sync"]["dependency_events"]["latest_event_id"], 7);
}

#[test]
fn an_unsupported_structural_event_forces_full_reconciliation() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "First");
    let initial = mock_full(
        &mut github,
        std::slice::from_ref(&one),
        &[(1, "[]")],
        event_page(vec![ordinary_event(100, 1)]),
        1,
    );
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    let since = support::replica_since(&state, "acme/widgets");

    let issues = mock_issue_delta(&mut github, &since, vec![one.clone()]);
    let comments = mock_comment_delta(&mut github, &since, Vec::new());
    let unsupported_events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .expect(2)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(event_page(vec![
            json!({
                "id": 101,
                "event": "dependency_reordered",
                "created_at": "2026-08-07T10:00:00Z",
                "issue": issue_reference(1)
            }),
            ordinary_event(100, 1),
        ]))
        .create();
    let reconciled = mock_full(&mut github, &[one], &[(1, "[]")], String::new(), 0);

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("unsupported event reconciliation"),
    );
    issues.assert();
    comments.assert();
    unsupported_events.assert();
    reconciled.assert_without_events();
    assert_eq!(
        support::load_replica(&state, "acme/widgets")["sync"]["dependency_events"]["latest_event_id"],
        101
    );
}

#[test]
fn a_remote_issue_count_drop_reconciles_a_deletion_missing_from_delta_feeds() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "Survives");
    let two = issue(2, "Deleted remotely");
    let initial = mock_full(
        &mut github,
        &[one.clone(), two],
        &[(1, "[]"), (2, "[]")],
        event_page(vec![ordinary_event(100, 1)]),
        1,
    );
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    let since = support::replica_since(&state, "acme/widgets");

    let issues = mock_issue_delta(&mut github, &since, vec![one.clone()]);
    let comments = mock_comment_delta(&mut github, &since, Vec::new());
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .expect(2)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(event_page(vec![ordinary_event(100, 1)]))
        .create();
    let count = support::mock_issue_count(&mut github, 1);
    let reconciled = mock_full(&mut github, &[one], &[(1, "[]")], String::new(), 0);

    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("deletion reconciliation"),
    );
    issues.assert();
    comments.assert();
    events.assert();
    count.assert();
    reconciled.assert_without_events();
    let replica = support::load_replica(&state, "acme/widgets");
    assert_eq!(replica["issues"].as_array().expect("Issues").len(), 1);
    assert!(replica_issue(&replica, 2).is_none());
}

#[test]
fn an_empty_event_feed_without_an_id_anchor_reconciles_fully_again() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let one = issue(1, "No event history");
    let initial = mock_full(
        &mut github,
        std::slice::from_ref(&one),
        &[(1, "[]")],
        "[]".to_owned(),
        1,
    );
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("initial sync"),
    );
    initial.assert();
    assert!(
        support::load_replica(&state, "acme/widgets")["sync"]["dependency_events"]
            .get("latest_event_id")
            .is_none()
    );

    let reconciled = mock_full(&mut github, &[one], &[(1, "[]")], "[]".to_owned(), 1);
    support::assert_success(
        &support::sync_command(&state, &github.url(), "acme/widgets")
            .output()
            .expect("unanchored reconciliation"),
    );
    reconciled.assert();
}

struct FullMocks {
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
    events: Option<Mock>,
}

impl FullMocks {
    fn assert(self) {
        self.assert_without_events();
    }

    fn assert_without_events(self) {
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
        if let Some(events) = self.events {
            events.assert();
        }
    }
}

fn mock_full(
    github: &mut Server,
    issues: &[Value],
    dependencies: &[(u64, &str)],
    events_body: String,
    event_expectation: usize,
) -> FullMocks {
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
        .with_body(serde_json::to_string(issues).expect("Issues"))
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependencies = dependencies
        .iter()
        .map(|(number, body)| {
            github
                .mock(
                    "GET",
                    format!("/repos/acme/widgets/issues/{number}/dependencies/blocked_by").as_str(),
                )
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(*body)
                .create()
        })
        .collect();
    let events = (event_expectation > 0).then(|| {
        github
            .mock("GET", "/repos/acme/widgets/issues/events")
            .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
            .expect(event_expectation)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(events_body)
            .create()
    });
    FullMocks {
        issues: issue_mock,
        comments,
        dependencies,
        events,
    }
}

fn mock_issue_delta(github: &mut Server, since: &str, issues: Vec<Value>) -> Mock {
    github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(support::issue_delta_query(since, None))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&issues).expect("Issue delta"))
        .create()
}

fn mock_comment_delta(github: &mut Server, since: &str, comments: Vec<Value>) -> Mock {
    github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(support::comment_delta_query(since, None))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(serde_json::to_string(&comments).expect("comment delta"))
        .create()
}

fn event_page(events: Vec<Value>) -> String {
    serde_json::to_string(&events).expect("event page")
}

fn ordinary_event(id: u64, issue_number: u64) -> Value {
    json!({
        "id": id,
        "event": "labeled",
        "created_at": "2026-08-06T10:00:00Z",
        "issue": issue_reference(issue_number)
    })
}

fn dependency_event(
    id: u64,
    event: &str,
    issue_number: u64,
    relationship_field: &str,
    related_number: u64,
) -> Value {
    let mut event = json!({
        "id": id,
        "event": event,
        "created_at": "2026-08-07T10:00:00Z",
        "issue": issue_reference(issue_number)
    });
    event[relationship_field] = issue_reference(related_number);
    event
}

fn issue_reference(number: u64) -> Value {
    json!({
        "number": number,
        "title": format!("Issue {number}"),
        "state": "open",
        "repository": { "full_name": "acme/widgets" }
    })
}

fn blocker(number: u64) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "repository_url": "https://api.github.com/repos/acme/widgets",
        "number": number,
        "state": "open"
    })
}

fn comment(id: u64, issue_number: u64) -> Value {
    json!({
        "id": id,
        "node_id": format!("IC_{id}"),
        "html_url": format!("https://github.com/acme/widgets/issues/{issue_number}#issuecomment-{id}"),
        "body": "Comment discovered with the Issue",
        "user": null,
        "author_association": "NONE",
        "created_at": "2026-08-07T09:00:00Z",
        "updated_at": "2026-08-07T09:00:00Z",
        "issue_url": format!("https://api.github.com/repos/acme/widgets/issues/{issue_number}")
    })
}

fn issue(number: u64, title: &str) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": title,
        "body": "Body",
        "state": "open",
        "state_reason": null,
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": { "login": "ada", "id": 10, "node_id": "U_ada" },
        "assignees": [],
        "labels": [],
        "created_at": "2026-08-01T10:00:00Z",
        "updated_at": "2026-08-02T11:00:00Z",
        "closed_at": null
    })
}

fn replica_issue(replica: &Value, number: u64) -> Option<&Value> {
    replica["issues"]
        .as_array()
        .expect("Issues")
        .iter()
        .find(|issue| issue["number"] == number)
}
