#![allow(dead_code)]

use std::{fs, process::Command};

use chrono::{DateTime, Duration, SecondsFormat};
use mockito::{Matcher, Mock, Server};
use serde_json::Value;
use tempfile::TempDir;

pub(crate) fn issue_delta_query(since: &str, page: Option<u64>) -> Matcher {
    let mut fields = vec![
        Matcher::UrlEncoded("state".into(), "all".into()),
        Matcher::UrlEncoded("sort".into(), "created".into()),
        Matcher::UrlEncoded("direction".into(), "asc".into()),
        Matcher::UrlEncoded("since".into(), since.into()),
        Matcher::UrlEncoded("per_page".into(), "100".into()),
    ];
    if let Some(page) = page {
        fields.push(Matcher::UrlEncoded("page".into(), page.to_string()));
    }
    Matcher::AllOf(fields)
}

#[allow(dead_code)]
pub(crate) fn comment_delta_query(since: &str, page: Option<u64>) -> Matcher {
    let mut fields = vec![
        Matcher::UrlEncoded("sort".into(), "created".into()),
        Matcher::UrlEncoded("direction".into(), "asc".into()),
        Matcher::UrlEncoded("since".into(), since.into()),
        Matcher::UrlEncoded("per_page".into(), "100".into()),
    ];
    if let Some(page) = page {
        fields.push(Matcher::UrlEncoded("page".into(), page.to_string()));
    }
    Matcher::AllOf(fields)
}

pub(crate) fn sync_command(state: &TempDir, api_url: &str, repository: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["sync", "--repo", repository, "--json"]);
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

pub(crate) fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn replica_path(state: &TempDir, repository: &str) -> std::path::PathBuf {
    state
        .path()
        .join("repositories")
        .join(repository)
        .join("replica.json")
}

pub(crate) fn load_replica(state: &TempDir, repository: &str) -> Value {
    serde_json::from_slice(&fs::read(replica_path(state, repository)).expect("Local replica"))
        .expect("replica JSON")
}

pub(crate) fn replica_since(state: &TempDir, repository: &str) -> String {
    let replica = load_replica(state, repository);
    let watermark = DateTime::parse_from_rfc3339(
        replica["sync"]["ordinary_issues"]["watermark"]
            .as_str()
            .expect("ordinary-Issue watermark"),
    )
    .expect("valid watermark");
    (watermark - Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn mock_issue_count(github: &mut Server, count: u64) -> Mock {
    github
        .mock("POST", "/graphql")
        .match_body(Matcher::Regex("IssueInventoryCount".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "data": { "repository": { "issues": { "totalCount": count } } }
            })
            .to_string(),
        )
        .create()
}
