use std::{fs, process::Command};

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn graph_generates_a_deterministic_valid_offline_site_without_raw_records() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site");
    fs::create_dir(&output_directory).expect("existing output directory");
    fs::write(output_directory.join("stale.txt"), "old site").expect("stale artifact");
    let mocks = mock_repository(
        &mut github,
        issue_inventory(),
        vec![
            (1, "[]".to_owned()),
            (2, blockers_for_two()),
            (3, "[]".to_owned()),
        ],
    );

    let generated = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("graph command");
    assert_success(&generated);
    let generated: Value = serde_json::from_slice(&generated.stdout).expect("graph output JSON");
    assert_eq!(generated["schema_version"], "grit.graph/v1");
    assert_eq!(generated["command"], "graph");
    assert_eq!(generated["repository"], "acme/widgets");
    assert_eq!(generated["source"], "live");
    assert_eq!(generated["artifact"]["node_count"], 4);
    assert_eq!(generated["artifact"]["edge_count"], 2);
    mocks.assert();

    assert!(!output_directory.join("stale.txt").exists());
    let graph_bytes = fs::read(output_directory.join("graph.json")).expect("graph JSON");
    let html_bytes = fs::read(output_directory.join("index.html")).expect("graph HTML");
    let stylesheet_bytes = fs::read(output_directory.join("app.css")).expect("graph stylesheet");
    let javascript_bytes = fs::read(output_directory.join("app.js")).expect("graph JavaScript");
    let schema_bytes = fs::read(output_directory.join("graph.schema.json")).expect("graph schema");
    let graph: Value = serde_json::from_slice(&graph_bytes).expect("artifact JSON");
    assert_eq!(graph["schema_version"], "grit.graph-artifact/v1");
    assert_eq!(graph["schema_url"], "./graph.schema.json");
    assert_eq!(graph["repository"], "acme/widgets");
    assert!(graph["synced_at"].as_str().is_some());
    assert!(graph["input_hash"].as_str().is_some());
    assert_eq!(graph["effective_input_hash"], graph["input_hash"]);
    assert!(graph["artifact_hash"].as_str().is_some());
    assert_eq!(graph["provenance"]["base"], "synchronized");
    assert_eq!(graph["provenance"]["pending_mutation_count"], 0);
    assert_eq!(graph["provenance"]["pending_operation_ids"], json!([]));
    assert_eq!(graph["operational_counts"]["operational_issue_count"], 2);
    assert_eq!(graph["operational_counts"]["ready_count"], 1);
    assert_eq!(graph["operational_counts"]["executable_count"], 1);
    assert_eq!(graph["operational_counts"]["blocked_count"], 1);
    assert_eq!(
        node_keys(&graph),
        vec![
            "acme/widgets#1",
            "acme/widgets#2",
            "acme/widgets#3",
            "partners/platform#42"
        ]
    );
    assert_eq!(graph["nodes"][0]["readiness"], "ready");
    assert_eq!(graph["nodes"][1]["readiness"], "blocked");
    assert_eq!(graph["nodes"][2]["readiness"], "closed");
    assert_eq!(graph["nodes"][3]["kind"], "external_blocker");
    assert_eq!(graph["nodes"][3]["readiness"], "external_open");
    assert_eq!(graph["nodes"][0]["position"]["layer"], 0);
    assert_eq!(graph["nodes"][1]["position"]["layer"], Value::Null);
    assert_eq!(
        graph["edges"],
        json!([
            {
                "blocked": "acme/widgets#2",
                "blocker": "acme/widgets#1",
                "kind": "blocked_by",
                "provenance": {
                    "state": "synchronized",
                    "operation_ids": []
                }
            },
            {
                "blocked": "acme/widgets#2",
                "blocker": "partners/platform#42",
                "kind": "blocked_by",
                "provenance": {
                    "state": "synchronized",
                    "operation_ids": []
                }
            }
        ])
    );
    let serialized = String::from_utf8(graph_bytes.clone()).expect("UTF-8 graph JSON");
    assert!(!serialized.contains("private body"));
    assert!(!serialized.contains("private comment"));
    assert!(!serialized.contains("operation-123"));

    let html = String::from_utf8(html_bytes.clone()).expect("UTF-8 HTML");
    assert!(html.contains("<table"));
    assert!(html.contains("<caption>Issue graph for acme/widgets</caption>"));
    assert!(html.contains("Root &lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("area:&lt;img src=x onerror=alert(2)&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<img src=x onerror=alert(2)>"));
    assert!(!html.contains("<script src=\"http"));
    assert!(!html.contains("<link rel=\"stylesheet\" href=\"http"));
    assert!(html.contains("href=\"./graph.json\""));
    assert!(html.contains("connect-src 'none'"));
    assert!(html.contains("src=\"./app.js\""));
    assert!(html.contains("href=\"./app.css\""));
    assert!(!String::from_utf8_lossy(&javascript_bytes).contains("fetch("));
    assert!(!String::from_utf8_lossy(&stylesheet_bytes).contains("url(http"));

    let schema: Value = serde_json::from_slice(&schema_bytes).expect("schema JSON");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["$defs"]["node"]["additionalProperties"], false);
    assert_eq!(schema["$defs"]["edge"]["additionalProperties"], false);
    let schema_text = String::from_utf8(schema_bytes.clone()).expect("UTF-8 schema");
    assert!(schema_text.contains("pending"));
    assert!(schema_text.contains("operation_ids"));

    let regenerated = graph_command(&state, &github.url(), &output_directory)
        .env_remove("GH_TOKEN")
        .output()
        .expect("offline graph regeneration");
    assert_success(&regenerated);
    let regenerated: Value =
        serde_json::from_slice(&regenerated.stdout).expect("graph output JSON");
    assert_eq!(regenerated["source"], "local_fallback");
    assert_eq!(
        fs::read(output_directory.join("graph.json")).expect("regenerated graph"),
        graph_bytes
    );
    assert_eq!(
        fs::read(output_directory.join("index.html")).expect("regenerated HTML"),
        html_bytes
    );
    assert_eq!(
        fs::read(output_directory.join("graph.schema.json")).expect("regenerated schema"),
        schema_bytes
    );
    assert_eq!(
        fs::read(output_directory.join("app.css")).expect("regenerated stylesheet"),
        stylesheet_bytes
    );
    assert_eq!(
        fs::read(output_directory.join("app.js")).expect("regenerated JavaScript"),
        javascript_bytes
    );
}

#[test]
fn dependency_layers_follow_readiness_and_leave_cycles_unresolved() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site");
    let issues = json!([
        issue(1, "Root", "open"),
        issue(2, "Disconnected root", "open"),
        issue(3, "Satisfied history", "closed"),
        issue(4, "Ready behind closed history", "open"),
        issue(5, "First dependent", "open"),
        issue(6, "AND dependent", "open"),
        issue(7, "Cycle A", "open"),
        issue(8, "Cycle B", "open"),
        issue(9, "Behind cycle", "open")
    ])
    .to_string();
    let no_dependencies = "[]".to_owned();
    let mocks = mock_repository(
        &mut github,
        issues,
        vec![
            (1, no_dependencies.clone()),
            (2, no_dependencies.clone()),
            (3, no_dependencies),
            (4, blocker_list(&[(3, "closed")])),
            (5, internal_blocker(1)),
            (6, blocker_list(&[(1, "open"), (5, "open")])),
            (7, internal_blocker(8)),
            (8, internal_blocker(7)),
            (9, internal_blocker(7)),
        ],
    );

    let output = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("graph command");
    assert_success(&output);
    let graph: Value = serde_json::from_slice(
        &fs::read(output_directory.join("graph.json")).expect("graph artifact"),
    )
    .expect("graph JSON");

    assert_eq!(layer(&graph, 1), Some(0));
    assert_eq!(layer(&graph, 2), Some(0));
    assert_eq!(layer(&graph, 3), None);
    assert_eq!(layer(&graph, 4), Some(0));
    assert_eq!(layer(&graph, 5), Some(1));
    assert_eq!(layer(&graph, 6), Some(2));
    assert_eq!(layer(&graph, 7), None);
    assert_eq!(layer(&graph, 8), None);
    assert_eq!(layer(&graph, 9), None);
    mocks.assert();
}

#[test]
#[ignore = "requires a Chrome-compatible browser; run explicitly as documented in README.md"]
fn generated_site_is_a_keyboard_accessible_offline_graph_explorer() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site");
    let mocks = mock_repository(
        &mut github,
        browser_issue_inventory(),
        vec![
            (1, "[]".to_owned()),
            (2, blockers_for_two()),
            (3, "[]".to_owned()),
            (4, internal_blocker(5)),
            (5, internal_blocker(4)),
        ],
    );
    let generated = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("graph command");
    assert_success(&generated);
    mocks.assert();

    let harness = workspace.path().join("browser-test.html");
    fs::write(
        &harness,
        "<!doctype html><html><body><iframe id=\"app\" src=\"./site/index.html\"></iframe><output id=\"result\">pending</output><script src=\"./browser-test.js\"></script></body></html>\n",
    )
    .expect("browser harness");
    fs::write(
        workspace.path().join("browser-test.js"),
        include_str!("fixtures/graph_browser_harness.js"),
    )
    .expect("browser harness JavaScript");

    let browser_profile = TempDir::new().expect("temporary browser profile");
    let browser_binary = std::env::var_os("GRIT_BROWSER").unwrap_or_else(|| "google-chrome".into());
    let browser = Command::new(browser_binary)
        .arg("--headless=new")
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-default-apps")
        .arg("--disable-sync")
        .arg("--metrics-recording-only")
        .arg("--no-first-run")
        .arg("--allow-file-access-from-files")
        .arg("--host-resolver-rules=MAP * ~NOTFOUND")
        .arg("--virtual-time-budget=3000")
        .arg(format!(
            "--user-data-dir={}",
            browser_profile.path().display()
        ))
        .arg("--dump-dom")
        .arg(format!("file://{}", harness.display()))
        .output()
        .expect("launch Google Chrome");
    assert!(
        browser.status.success(),
        "Chrome stderr: {}",
        String::from_utf8_lossy(&browser.stderr)
    );
    let dom = String::from_utf8(browser.stdout).expect("browser DOM");
    let result = browser_result(&dom);
    assert!(result.get("error").is_none(), "browser result: {result}");
    let checks = result["checks"].as_object().expect("browser checks");
    let expected_checks = [
        "title_search",
        "number_search",
        "side_panel",
        "canonical_link",
        "precomputed_position",
        "scc_position",
        "labels_hidden_by_default",
        "selected_label_only",
        "keyboard_navigation",
        "zoom",
        "hostile_text_is_literal",
        "no_injected_elements",
    ];
    assert_eq!(
        checks.len(),
        expected_checks.len(),
        "browser result: {result}"
    );
    for name in expected_checks {
        assert_eq!(
            checks.get(name),
            Some(&Value::Bool(true)),
            "browser check {name} failed: {result}"
        );
    }
}

#[test]
fn a_dangling_internal_dependency_fails_without_replacing_the_previous_site() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site");
    fs::create_dir(&output_directory).expect("existing output directory");
    fs::write(output_directory.join("sentinel.txt"), "previous valid site")
        .expect("previous artifact");
    let mocks = mock_repository(
        &mut github,
        json!([issue(2, "Dangling dependent", "open")]).to_string(),
        vec![(2, internal_blocker(1))],
    );

    let output = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("invalid graph command");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("dangling internal Dependency endpoint"));
    assert!(stderr.contains("acme/widgets#1"));
    assert_eq!(
        fs::read_to_string(output_directory.join("sentinel.txt")).expect("preserved previous site"),
        "previous valid site"
    );
    assert!(!output_directory.join("graph.json").exists());
    mocks.assert();
}

fn graph_command(state: &TempDir, api_url: &str, output: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["graph", "--repo", "acme/widgets", "--output"]);
    command.arg(output);
    command.arg("--json");
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    command
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

fn mock_repository(
    github: &mut Server,
    issues_body: String,
    dependencies: Vec<(u64, String)>,
) -> RepositoryMocks {
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
        .with_body(issues_body)
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!([comment(101, 1)]).to_string())
        .create();
    let dependencies = dependencies
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
        labels,
        events,
        issues,
        comments,
        dependencies,
    }
}

fn issue_inventory() -> String {
    json!([
        issue(
            1,
            "Root <script>alert(1)</script><!-- grit:operation operation-123 -->",
            "open"
        ),
        issue(2, "Dependent", "open"),
        issue(3, "Historical", "closed")
    ])
    .to_string()
}

fn browser_issue_inventory() -> String {
    json!([
        issue(
            1,
            "Root <script>alert(1)</script><!-- grit:operation operation-123 -->",
            "open"
        ),
        issue(2, "Dependent", "open"),
        issue(3, "Historical", "closed"),
        issue(4, "Cycle A", "open"),
        issue(5, "Cycle B", "open")
    ])
    .to_string()
}

fn issue(number: u64, title: &str, state: &str) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": title,
        "body": "private body <!-- grit:operation operation-123 -->",
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": null,
        "assignees": [],
        "labels": [{"id": 9000 + number, "node_id": format!("L_{number}"), "name": "area:<img src=x onerror=alert(2)>", "color": "123456", "description": null}],
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": if state == "closed" { Some("2026-08-02T00:00:00Z") } else { None }
    })
}

fn comment(id: u64, issue_number: u64) -> Value {
    json!({
        "id": id,
        "node_id": format!("IC_{id}"),
        "html_url": format!("https://github.com/acme/widgets/issues/{issue_number}#issuecomment-{id}"),
        "issue_url": format!("https://api.github.com/repos/acme/widgets/issues/{issue_number}"),
        "body": "private comment <!-- grit:operation operation-123 -->",
        "user": null,
        "author_association": "NONE",
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z"
    })
}

fn blockers_for_two() -> String {
    json!([
        blocker(
            100,
            "I_1",
            "https://api.github.com/repos/acme/widgets",
            1,
            "open"
        ),
        blocker(
            4200,
            "I_42",
            "https://api.github.com/repos/partners/platform",
            42,
            "open"
        )
    ])
    .to_string()
}

fn internal_blocker(number: u64) -> String {
    json!([blocker(
        number * 100,
        &format!("I_{number}"),
        "https://api.github.com/repos/acme/widgets",
        number,
        "open"
    )])
    .to_string()
}

fn blocker_list(blockers: &[(u64, &str)]) -> String {
    Value::Array(
        blockers
            .iter()
            .map(|(number, state)| {
                blocker(
                    number * 100,
                    &format!("I_{number}"),
                    "https://api.github.com/repos/acme/widgets",
                    *number,
                    state,
                )
            })
            .collect(),
    )
    .to_string()
}

fn blocker(id: u64, node_id: &str, repository_url: &str, number: u64, state: &str) -> Value {
    json!({
        "id": id,
        "node_id": node_id,
        "repository_url": repository_url,
        "number": number,
        "state": state
    })
}

fn node_keys(graph: &Value) -> Vec<&str> {
    graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| node["key"].as_str().expect("node key"))
        .collect()
}

fn layer(graph: &Value, number: u64) -> Option<u64> {
    graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["number"] == number)
        .expect("node")
        .get("position")
        .and_then(|position| position.get("layer"))
        .and_then(Value::as_u64)
}

fn browser_result(dom: &str) -> Value {
    let marker = "<output id=\"result\">";
    let start = dom.find(marker).expect("browser result element") + marker.len();
    let end = dom[start..]
        .find("</output>")
        .map(|offset| start + offset)
        .expect("browser result closing tag");
    serde_json::from_str(&dom[start..end]).expect("browser result JSON")
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
