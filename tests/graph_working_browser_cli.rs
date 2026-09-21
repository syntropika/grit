use std::{fs, process::Command};

use mockito::Server;
use serde_json::{Value, json};
use tempfile::TempDir;

mod support;

#[test]
#[ignore = "requires a Chrome-compatible browser; run with GRIT_BROWSER set"]
fn pending_draft_exploration_preserves_identity_filters_and_recommendation_evidence() {
    let state = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let mut github = Server::new();
    let mocks = support::mock_repository(
        &mut github,
        "acme/widgets",
        vec![
            support::issue(1, "open", &["priority:p3"], &[]),
            support::issue(2, "open", &["priority:p4"], &[]),
        ],
        vec![(1, vec![]), (2, vec![])],
    );
    execute(
        &state,
        &github.url(),
        &["sync", "--repo", "acme/widgets", "--json"],
    );
    mocks.assert();
    let unavailable = Server::new();
    let api = unavailable.url();
    let created = execute(
        &state,
        &api,
        &[
            "create",
            "--repo",
            "acme/widgets",
            "--title",
            "Initial Draft",
            "--json",
        ],
    );
    let key = created["draft"]["key"].as_str().unwrap();
    execute(
        &state,
        &api,
        &["update", key, "--title", "Edited local Draft", "--json"],
    );
    execute(&state, &api, &["update", key, "--priority", "p0", "--json"]);
    execute(
        &state,
        &api,
        &["block", "acme/widgets#2", "--by", key, "--json"],
    );
    let site = workspace.path().join("site");
    execute(
        &state,
        &api,
        &[
            "graph",
            "--repo",
            "acme/widgets",
            "--output",
            site.to_str().unwrap(),
            "--json",
        ],
    );

    // Exercise the existing constrained-view policy on this small real Working graph.
    let index = site.join("index.html");
    let html = fs::read_to_string(&index).unwrap();
    let marker = "id=\"graph-presentation-data\"";
    let start = html.find(marker).unwrap();
    let data_start = start + html[start..].find('>').unwrap() + 1;
    let data_end = data_start + html[data_start..].find("</script>").unwrap();
    let mut presentation: Value = serde_json::from_str(&html[data_start..data_end]).unwrap();
    presentation["mode"] = json!("constrained");
    presentation["initial_network_node_limit"] = json!(1);
    presentation["initial_node_keys"] = json!(["acme/widgets#1"]);
    fs::write(
        &index,
        format!(
            "{}{}{}",
            &html[..data_start],
            presentation,
            &html[data_end..]
        ),
    )
    .unwrap();

    let harness = workspace.path().join("browser-test.html");
    fs::write(&harness, "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Pending graph browser checks</title></head><body><iframe id=\"app\" src=\"site/index.html\"></iframe><pre id=\"result\">pending</pre><script src=\"harness.js\"></script></body></html>").unwrap();
    fs::write(
        workspace.path().join("harness.js"),
        include_str!("fixtures/graph_working_browser_harness.js"),
    )
    .unwrap();
    let browser = std::env::var_os("GRIT_BROWSER").unwrap_or_else(|| "google-chrome".into());
    let audit =
        support::browser::audit_local_page(&browser, &workspace.path().join("profile"), &harness);
    assert!(audit.result.get("error").is_none(), "{}", audit.result);
    let checks = audit.result["checks"].as_object().expect("browser checks");
    assert!(
        checks.len() >= 10,
        "all Working graph scenarios must execute"
    );
    for (name, passed) in checks {
        assert_eq!(passed, true, "browser check {name}: {}", audit.result);
    }
    assert!(
        audit
            .requests
            .iter()
            .all(|url| !url.starts_with("http:") && !url.starts_with("https:")),
        "offline graph made a network request: {:?}",
        audit.requests
    );
}

fn execute(state: &TempDir, api: &str, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_grit"))
        .args(args)
        .env("GH_TOKEN", "local-fixture-token")
        .env("GRIT_GITHUB_API_URL", api)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "")
        .output()
        .unwrap();
    support::assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}
