use std::{fs, process::Command};

use chrono::{DateTime, Duration, SecondsFormat};
use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

mod support;

const INITIAL_WATERMARK: &str = "2026-08-02T11:00:00Z";

#[test]
fn incremental_sync_upserts_old_and_new_issues_with_every_ordinary_change() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let initial_issue = issue(
        7,
        "open",
        "Initial title",
        "Initial body",
        INITIAL_WATERMARK,
    );
    let initial = mock_initial(&mut github, vec![initial_issue.clone()], &[7]);

    let first = sync_command(&state, &github.url())
        .output()
        .expect("initial sync");
    assert_success(&first);
    initial.assert();
    let initial_since = replica_since(&state);

    let closed = issue_with_classification(
        7,
        "closed",
        "Closed after review",
        "Revised body",
        "2099-08-07T12:00:00Z",
        vec![actor("grace", 11)],
        vec![label("priority:p0", 21)],
    );
    let created = issue(10, "open", "New Issue", "New work", "2099-08-07T12:15:00Z");
    let changed_issues = mock_issue_delta(
        &mut github,
        &initial_since,
        None,
        200,
        serde_json::to_string(&vec![closed, created]).expect("Issue delta"),
        None,
    );
    let changed_comments = mock_comment_delta(
        &mut github,
        &initial_since,
        None,
        200,
        serde_json::to_string(&vec![comment(
            701,
            7,
            "A newly synchronized comment",
            "2099-08-07T12:30:00Z",
        )])
        .expect("comment delta"),
        None,
    );
    let dependency_seven = mock_dependencies(&mut github, 7);
    let dependency_ten = mock_dependencies(&mut github, 10);

    let second = sync_command(&state, &github.url())
        .output()
        .expect("incremental sync");
    assert_success(&second);
    changed_issues.assert();
    changed_comments.assert();
    dependency_seven.assert();
    dependency_ten.assert();

    let replica = load_replica(&state);
    let watermark = DateTime::parse_from_rfc3339(
        replica["sync"]["ordinary_issues"]["watermark"]
            .as_str()
            .expect("watermark"),
    )
    .expect("valid watermark");
    let synced_at =
        DateTime::parse_from_rfc3339(replica["synced_at"].as_str().expect("Synchronization time"))
            .expect("valid Synchronization time");
    assert!(watermark <= synced_at);
    assert_ne!(
        replica["sync"]["ordinary_issues"]["watermark"],
        "2099-08-07T12:30:00Z"
    );
    assert_eq!(replica["issues"].as_array().expect("Issues").len(), 2);
    let issue_seven = replica_issue(&replica, 7);
    assert_eq!(issue_seven["state"], "closed");
    assert_eq!(issue_seven["title"], "Closed after review");
    assert_eq!(issue_seven["body"], "Revised body");
    assert_eq!(issue_seven["assignees"][0]["login"], "grace");
    assert_eq!(issue_seven["labels"][0]["name"], "priority:p0");
    assert_eq!(
        issue_seven["comments"][0]["body"],
        "A newly synchronized comment"
    );
    assert_eq!(replica_issue(&replica, 10)["title"], "New Issue");

    let second_since = replica_since(&state);
    let reopened = issue_with_classification(
        7,
        "open",
        "Reopened old Issue",
        "Follow-up required",
        "2099-08-07T13:00:00Z",
        Vec::new(),
        vec![label("priority:p1", 22)],
    );
    let reopened_issues = mock_issue_delta(
        &mut github,
        &second_since,
        None,
        200,
        serde_json::to_string(&vec![reopened]).expect("reopened Issue delta"),
        None,
    );
    let no_new_comments =
        mock_comment_delta(&mut github, &second_since, None, 200, "[]".to_owned(), None);
    let reopened_dependencies = mock_dependencies(&mut github, 7);

    let third = sync_command(&state, &github.url())
        .output()
        .expect("reopening sync");
    assert_success(&third);
    reopened_issues.assert();
    no_new_comments.assert();
    reopened_dependencies.assert();

    let replica = load_replica(&state);
    let issue_seven = replica_issue(&replica, 7);
    assert_eq!(issue_seven["state"], "open");
    assert_eq!(issue_seven["title"], "Reopened old Issue");
    assert_eq!(issue_seven["assignees"], json!([]));
    assert_eq!(issue_seven["labels"][0]["name"], "priority:p1");
    assert_eq!(
        issue_seven["comments"][0]["body"],
        "A newly synchronized comment"
    );
    assert_eq!(
        issue_seven["comments"]
            .as_array()
            .expect("Issue comments")
            .len(),
        1
    );
}

#[test]
fn unchanged_sync_uses_only_safe_query_scoped_conditional_requests() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let initial_issue = issue(7, "open", "Stable Issue", "No changes", INITIAL_WATERMARK);
    let initial = mock_initial(&mut github, vec![initial_issue.clone()], &[7]);

    let first = sync_command(&state, &github.url())
        .output()
        .expect("initial sync");
    assert_success(&first);
    initial.assert();
    let initial_watermark = replica_watermark(&state);
    let initial_since = replica_since(&state);

    let warm_issues = mock_issue_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Missing),
        200,
        serde_json::to_string(&vec![initial_issue]).expect("overlapped Issue"),
        Some("\"issues-safe-v1\""),
    );
    let warm_comments = mock_comment_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Missing),
        200,
        "[]".to_owned(),
        Some("\"comments-safe-v1\""),
    );

    let second = sync_command(&state, &github.url())
        .output()
        .expect("warm conditional state");
    assert_success(&second);
    warm_issues.assert();
    warm_comments.assert();

    let unchanged_issues = mock_issue_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Exact("\"issues-safe-v1\"".into())),
        304,
        String::new(),
        None,
    );
    let unchanged_comments = mock_comment_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Exact("\"comments-safe-v1\"".into())),
        304,
        String::new(),
        None,
    );

    let third = sync_command(&state, &github.url())
        .output()
        .expect("unchanged conditional sync");
    assert_success(&third);
    unchanged_issues.assert();
    unchanged_comments.assert();

    let replica = load_replica(&state);
    assert_eq!(replica["issues"].as_array().expect("Issues").len(), 1);
    assert_eq!(replica_issue(&replica, 7)["title"], "Stable Issue");
    assert_eq!(
        replica["sync"]["ordinary_issues"]["watermark"],
        initial_watermark
    );
}

#[test]
fn a_paginated_etag_is_not_reused_as_a_global_continuity_signal() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let initial_issue = issue(7, "open", "Stable Issue", "No changes", INITIAL_WATERMARK);
    let initial = mock_initial(&mut github, vec![initial_issue.clone()], &[7]);
    let first = sync_command(&state, &github.url())
        .output()
        .expect("initial sync");
    assert_success(&first);
    initial.assert();
    let initial_since = replica_since(&state);

    let mut pull_request = issue(
        8,
        "open",
        "Unrelated pull request",
        "Excluded",
        INITIAL_WATERMARK,
    );
    pull_request["pull_request"] = json!({
        "url": "https://api.github.com/repos/acme/widgets/pulls/8"
    });
    let next = format!(
        "<{}/repos/acme/widgets/issues?state=all&sort=created&direction=asc&since={}&per_page=100&page=2>; rel=\"next\"",
        github.url(),
        initial_since
    );
    let paginated_first = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(support::issue_delta_query(&initial_since, None))
        .match_header("if-none-match", Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("etag", "\"page-one-only\"")
        .with_header("link", &next)
        .with_body(serde_json::to_string(&vec![pull_request]).expect("first page"))
        .create();
    let paginated_second = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(support::issue_delta_query(&initial_since, Some(2)))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let warm_comments = mock_comment_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Missing),
        200,
        "[]".to_owned(),
        Some("\"comments-safe-v1\""),
    );

    let second = sync_command(&state, &github.url())
        .output()
        .expect("paginated delta");
    assert_success(&second);
    paginated_first.assert();
    paginated_second.assert();
    warm_comments.assert();

    let next_issues = mock_issue_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Missing),
        200,
        serde_json::to_string(&vec![initial_issue]).expect("overlap"),
        None,
    );
    let next_comments = mock_comment_delta(
        &mut github,
        &initial_since,
        Some(Matcher::Exact("\"comments-safe-v1\"".into())),
        304,
        String::new(),
        None,
    );
    let third = sync_command(&state, &github.url())
        .output()
        .expect("post-pagination delta");
    assert_success(&third);
    next_issues.assert();
    next_comments.assert();
}

#[test]
fn interrupted_incremental_pagination_preserves_the_complete_replica_and_cursor() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let initial = mock_initial(
        &mut github,
        vec![issue(7, "open", "Complete", "Saved", INITIAL_WATERMARK)],
        &[7],
    );
    let first = sync_command(&state, &github.url())
        .output()
        .expect("initial sync");
    assert_success(&first);
    initial.assert();
    let initial_since = replica_since(&state);
    let replica_path = replica_path(&state);
    let before = fs::read(&replica_path).expect("complete replica");

    let next = format!(
        "<{}/repos/acme/widgets/issues?state=all&sort=created&direction=asc&since={}&per_page=100&page=2>; rel=\"next\"",
        github.url(),
        initial_since
    );
    let first_page = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(support::issue_delta_query(&initial_since, None))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("link", &next)
        .with_body(
            serde_json::to_string(&vec![issue(
                7,
                "closed",
                "Incomplete",
                "Must not publish",
                "2026-08-07T12:00:00Z",
            )])
            .expect("partial page"),
        )
        .create();
    let failed_page = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(support::issue_delta_query(&initial_since, Some(2)))
        .with_status(403)
        .with_header("content-type", "application/json")
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", "1786100000")
        .with_body("{\"message\":\"rate limit\"}")
        .create();

    let failed = sync_command(&state, &github.url())
        .output()
        .expect("interrupted incremental sync");
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("rate limit"));
    assert_eq!(fs::read(&replica_path).expect("preserved replica"), before);
    first_page.assert();
    failed_page.assert();
}

struct InitialMocks {
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
}

impl InitialMocks {
    fn assert(self) {
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
    }
}

fn mock_initial(
    github: &mut Server,
    issues: Vec<Value>,
    dependency_numbers: &[u64],
) -> InitialMocks {
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
        .with_header("etag", "\"full-inventory-only\"")
        .with_body(serde_json::to_string(&issues).expect("initial Issues"))
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("etag", "\"full-comments-only\"")
        .with_body("[]")
        .create();
    let dependencies = dependency_numbers
        .iter()
        .map(|number| mock_dependencies(github, *number))
        .collect();
    InitialMocks {
        issues,
        comments,
        dependencies,
    }
}

fn mock_issue_delta(
    github: &mut Server,
    since: &str,
    if_none_match: Option<Matcher>,
    status: usize,
    body: String,
    etag: Option<&str>,
) -> Mock {
    let mut mock = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(support::issue_delta_query(since, None));
    if let Some(matcher) = if_none_match {
        mock = mock.match_header("if-none-match", matcher);
    }
    mock = mock.with_status(status).with_body(body);
    if let Some(etag) = etag {
        mock = mock.with_header("etag", etag);
    }
    mock.create()
}

fn mock_comment_delta(
    github: &mut Server,
    since: &str,
    if_none_match: Option<Matcher>,
    status: usize,
    body: String,
    etag: Option<&str>,
) -> Mock {
    let mut mock = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(support::comment_delta_query(since, None));
    if let Some(matcher) = if_none_match {
        mock = mock.match_header("if-none-match", matcher);
    }
    mock = mock.with_status(status).with_body(body);
    if let Some(etag) = etag {
        mock = mock.with_header("etag", etag);
    }
    mock.create()
}

fn mock_dependencies(github: &mut Server, issue_number: u64) -> Mock {
    github
        .mock(
            "GET",
            format!("/repos/acme/widgets/issues/{issue_number}/dependencies/blocked_by").as_str(),
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create()
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

fn issue(number: u64, state: &str, title: &str, body: &str, updated_at: &str) -> Value {
    issue_with_classification(
        number,
        state,
        title,
        body,
        updated_at,
        Vec::new(),
        Vec::new(),
    )
}

fn issue_with_classification(
    number: u64,
    state: &str,
    title: &str,
    body: &str,
    updated_at: &str,
    assignees: Vec<Value>,
    labels: Vec<Value>,
) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": title,
        "body": body,
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": actor("ada", 10),
        "assignees": assignees,
        "labels": labels,
        "created_at": "2026-08-01T10:00:00Z",
        "updated_at": updated_at,
        "closed_at": if state == "closed" { Some(updated_at) } else { None }
    })
}

fn actor(login: &str, id: u64) -> Value {
    json!({"login": login, "id": id, "node_id": format!("U_{login}")})
}

fn label(name: &str, id: u64) -> Value {
    json!({
        "id": id,
        "node_id": format!("L_{id}"),
        "name": name,
        "color": "123456",
        "description": null
    })
}

fn comment(id: u64, issue_number: u64, body: &str, updated_at: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("IC_{id}"),
        "html_url": format!("https://github.com/acme/widgets/issues/{issue_number}#issuecomment-{id}"),
        "body": body,
        "user": actor("commenter", 13),
        "author_association": "CONTRIBUTOR",
        "created_at": "2026-08-05T10:00:00Z",
        "updated_at": updated_at,
        "issue_url": format!("https://api.github.com/repos/acme/widgets/issues/{issue_number}")
    })
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn replica_path(state: &TempDir) -> std::path::PathBuf {
    state.path().join("repositories/acme/widgets/replica.json")
}

fn load_replica(state: &TempDir) -> Value {
    serde_json::from_slice(&fs::read(replica_path(state)).expect("Local replica"))
        .expect("replica JSON")
}

fn replica_watermark(state: &TempDir) -> String {
    load_replica(state)["sync"]["ordinary_issues"]["watermark"]
        .as_str()
        .expect("ordinary-Issue watermark")
        .to_owned()
}

fn replica_since(state: &TempDir) -> String {
    let watermark = replica_watermark(state);
    let watermark = DateTime::parse_from_rfc3339(&watermark).expect("valid watermark");
    (watermark - Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn replica_issue(replica: &Value, number: u64) -> &Value {
    replica["issues"]
        .as_array()
        .expect("Issues")
        .iter()
        .find(|issue| issue["number"] == number)
        .expect("Issue in replica")
}
