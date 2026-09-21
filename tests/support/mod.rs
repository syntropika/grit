use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};

pub struct RepositoryMocks {
    labels: Mock,
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
}

impl RepositoryMocks {
    pub fn assert(self) {
        self.labels.assert();
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
    }
}

pub fn mock_repository(
    github: &mut Server,
    repository: &str,
    issues: Vec<Value>,
    dependencies: Vec<(u64, Vec<Value>)>,
) -> RepositoryMocks {
    mock_repository_with_calls(github, repository, issues, dependencies, 1)
}

pub fn mock_repository_with_calls(
    github: &mut Server,
    repository: &str,
    issues: Vec<Value>,
    dependencies: Vec<(u64, Vec<Value>)>,
    expected_calls: usize,
) -> RepositoryMocks {
    let labels = github
        .mock("GET", format!("/repos/{repository}/labels").as_str())
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(canonical_labels().to_string())
        .expect(expected_calls)
        .create();
    let issues = issues
        .into_iter()
        .map(|mut issue| {
            if issue["html_url"]
                .as_str()
                .is_some_and(|url| url.starts_with("https://github.com/acme/placeholder/issues/"))
            {
                issue["html_url"] = Value::String(format!(
                    "https://github.com/{repository}/issues/{}",
                    issue["number"]
                        .as_u64()
                        .expect("fixture Issue has a number")
                ));
            }
            issue
        })
        .collect();
    let issues = github
        .mock("GET", format!("/repos/{repository}/issues").as_str())
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(Value::Array(issues).to_string())
        .expect(expected_calls)
        .create();
    let comments = github
        .mock(
            "GET",
            format!("/repos/{repository}/issues/comments").as_str(),
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(expected_calls)
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
            github
                .mock(
                    "GET",
                    format!("/repos/{repository}/issues/{number}/dependencies/blocked_by").as_str(),
                )
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(Value::Array(blockers).to_string())
                .expect(expected_calls)
                .create()
        })
        .collect();
    RepositoryMocks {
        labels,
        issues,
        comments,
        dependencies,
    }
}

pub fn issue(number: u64, state: &str, priority_labels: &[&str], assignees: &[&str]) -> Value {
    issue_for(
        "acme/placeholder",
        number,
        state,
        priority_labels,
        assignees,
    )
}

pub fn issue_for(
    repository: &str,
    number: u64,
    state: &str,
    priority_labels: &[&str],
    assignees: &[&str],
) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://github.com/{repository}/issues/{number}"),
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

pub fn internal_blocker(number: u64, state: &str) -> Value {
    internal_blocker_for("acme/placeholder", number, state)
}

pub fn internal_blocker_for(repository: &str, number: u64, state: &str) -> Value {
    blocker(repository, number, state)
}

pub fn external_blocker(repository: &str, number: u64, state: &str) -> Value {
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

pub fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
pub(crate) mod browser;
