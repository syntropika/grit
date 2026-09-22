use std::{fs, process::Command};

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn graph_embeds_the_exact_next_and_plan_evidence_for_one_effective_input() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site");
    let issues = json!([
        priority_issue(1, "Unlocking root", "open", "priority:p1"),
        priority_issue(2, "Independent runner-up", "open", "priority:p4"),
        issue(3, "Unlocked outcome", "open"),
        issue(4, "Cycle A", "open"),
        issue(5, "Cycle B", "open"),
        issue(6, "Opaque boundary", "open")
    ])
    .to_string();
    let mocks = mock_repository(
        &mut github,
        issues,
        vec![
            (1, "[]".to_owned()),
            (2, "[]".to_owned()),
            (3, internal_blocker(1)),
            (4, internal_blocker(5)),
            (5, internal_blocker(4)),
            (
                6,
                json!([blocker(
                    9900,
                    "I_99",
                    "https://api.github.com/repos/partners/private",
                    99,
                    "unknown"
                )])
                .to_string(),
            ),
        ],
    );

    let generated = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("graph command");
    assert_success(&generated);
    mocks.assert();
    let graph: Value = serde_json::from_slice(
        &fs::read(output_directory.join("graph.json")).expect("graph artifact"),
    )
    .expect("graph JSON");
    let next = offline_analysis_command(&state, &github.url(), "next")
        .output()
        .expect("offline next command");
    assert_success(&next);
    let next: Value = serde_json::from_slice(&next.stdout).expect("next JSON");
    let plan = offline_analysis_command(&state, &github.url(), "plan")
        .output()
        .expect("offline plan command");
    assert_success(&plan);
    let plan: Value = serde_json::from_slice(&plan.stdout).expect("plan JSON");

    assert_eq!(graph["effective_input_hash"], next["input_hash"]);
    assert_eq!(graph["analysis"]["policy_version"], next["policy_version"]);
    assert_eq!(graph["analysis"]["next"], analysis_projection(&next));
    assert_eq!(graph["analysis"]["plan"]["decision"], plan["decision"]);
    assert_eq!(
        graph["analysis"]["plan"]["parallel_now"],
        plan["parallel_now"]
    );
    assert_eq!(
        graph["analysis"]["plan"]["dependency_layers"],
        plan["dependency_layers"]
    );
    assert_eq!(
        graph["analysis"]["next"]["recommendation"]["first_issue"]["number"],
        1
    );
    assert_eq!(
        graph["analysis"]["next"]["comparison_to_runner_up"]["runner_up"]["number"],
        2
    );
    assert_eq!(
        graph["analysis"]["next"]["comparison_to_runner_up"]["message"],
        "it unlocks work earlier"
    );
    assert_eq!(
        graph["analysis"]["plan"]["dependency_layers"]["unresolved"]
            .as_array()
            .expect("unresolved Issues")
            .len(),
        3
    );
}

#[test]
fn closed_outcomes_are_visible_without_entering_operational_work() {
    let mut github = Server::new();
    let state = TempDir::new().expect("state directory");
    let workspace = TempDir::new().expect("graph workspace");
    let mut not_planned = issue(3, "Discarded proposal", "closed");
    not_planned["state_reason"] = json!("not_planned");
    let mut unknown_closure = issue(4, "Legacy closure", "closed");
    unknown_closure["state_reason"] = Value::Null;
    let mocks = mock_repository(
        &mut github,
        json!([
            issue(1, "Current work", "open"),
            issue(2, "Completed <script>unsafe()</script>", "closed"),
            not_planned,
            unknown_closure
        ])
        .to_string(),
        vec![
            (1, "[]".to_owned()),
            (2, "[]".to_owned()),
            (3, blocker_list(&[(2, "closed")])),
            (4, blocker_list(&[(3, "closed")])),
        ],
    );
    let output = graph_command(&state, &github.url(), workspace.path())
        .output()
        .expect("graph command");
    assert_success(&output);
    mocks.assert();
    let graph: Value =
        serde_json::from_slice(&fs::read(workspace.path().join("graph.json")).expect("graph JSON"))
            .expect("graph artifact");
    assert!(graph["nodes"][0].get("resolution").is_none());
    assert_eq!(graph["nodes"][1]["resolution"], "completed");
    assert_eq!(graph["nodes"][2]["resolution"], "not_planned");
    assert_eq!(graph["nodes"][3]["resolution"], "other");
    assert_eq!(
        graph["nodes"][0]["common"]["position"],
        json!({"layer":0,"x":0,"y":0})
    );
    for node in &graph["nodes"].as_array().unwrap()[1..] {
        assert!(node["common"]["position"]["layer"].is_null());
        assert_eq!(node["common"]["position"]["y"], 144);
    }
    assert_eq!(graph["nodes"][1]["common"]["position"]["x"], 0);
    assert_eq!(graph["nodes"][2]["common"]["position"]["x"], 320);
    assert_eq!(graph["nodes"][3]["common"]["position"]["x"], 640);
    assert_eq!(graph["operational_counts"]["operational_issue_count"], 1);
    assert_eq!(graph["operational_counts"]["ready_count"], 1);
    assert_eq!(
        graph["analysis"]["next"]["recommendation"]["first_issue"]["number"],
        1
    );
    let html = fs::read_to_string(workspace.path().join("index.html")).expect("HTML");
    assert!(html.contains("id=\"count-completed\">1</strong>"));
    assert!(html.contains("id=\"count-not-planned\">1</strong>"));
    assert!(html.contains("id=\"count-other\">1</strong>"));
    assert!(html.contains("<td>History</td>"));
    let completed_list = html
        .split("<ul id=\"completion-list\">")
        .nth(1)
        .expect("completed list")
        .split("</ul>")
        .next()
        .unwrap();
    assert!(completed_list.contains("Completed &lt;script&gt;unsafe()&lt;/script&gt;"));
    assert!(!completed_list.contains("Discarded proposal"));
    assert!(!completed_list.contains("Legacy closure"));
    assert!(!html.contains("<script>unsafe()</script>"));
}

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
    assert_eq!(generated["schema_version"], "hyfa.graph/v1");
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
    let network_view_bytes =
        fs::read(output_directory.join("network-view.js")).expect("network-view JavaScript");
    let graph_query_bytes =
        fs::read(output_directory.join("graph-query.js")).expect("graph query JavaScript");
    let javascript_bytes = fs::read(output_directory.join("app.js")).expect("graph JavaScript");
    let schema_bytes = fs::read(output_directory.join("graph.schema.json")).expect("graph schema");
    let graph: Value = serde_json::from_slice(&graph_bytes).expect("artifact JSON");
    assert_eq!(graph["schema_version"], "hyfa.graph-artifact/v3");
    assert_eq!(graph["schema_url"], "./graph.schema.json");
    assert_eq!(graph["repository"], "acme/widgets");
    assert!(graph["synced_at"].as_str().is_some());
    assert!(graph["input_hash"].as_str().is_some());
    assert_ne!(graph["effective_input_hash"], graph["input_hash"]);
    assert_eq!(
        graph["effective_input_hash"],
        graph["analysis"]["next"]["input_hash"]
    );
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
    assert_eq!(graph["nodes"][0]["status"], "ready");
    assert_eq!(graph["nodes"][1]["status"], "blocked");
    assert_eq!(graph["nodes"][2]["status"], "closed");
    assert_eq!(graph["nodes"][3]["kind"], "external_blocker");
    assert_eq!(graph["nodes"][3]["status"], "external_open");
    assert_eq!(graph["nodes"][0]["common"]["position"]["layer"], 0);
    assert_eq!(
        graph["nodes"][1]["common"]["position"]["layer"],
        Value::Null
    );
    assert!(
        graph["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .all(|node| node.get("projects").is_none())
    );
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
    assert!(html.contains("class=\"zoom-controls\" role=\"group\""));
    assert!(html.contains("id=\"graph-canvas\" data-zoom=\"1\" role=\"group\""));
    assert!(html.contains("Root &lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("area:&lt;img src=x onerror=alert(2)&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<img src=x onerror=alert(2)>"));
    assert!(!html.contains("<script src=\"http"));
    assert!(!html.contains("<link rel=\"stylesheet\" href=\"http"));
    assert!(html.contains("href=\"./graph.json\""));
    assert!(html.contains("connect-src 'none'"));
    assert!(html.contains("id=\"graph-presentation-data\""));
    assert!(html.contains("hyfa.graph-presentation/v1"));
    assert!(html.contains("\"mode\":\"full\""));
    assert!(html.contains("src=\"./network-view.js\""));
    assert!(html.contains("src=\"./app.js\""));
    assert!(html.contains("href=\"./app.css\""));
    assert!(!String::from_utf8_lossy(&javascript_bytes).contains("fetch("));
    assert!(!String::from_utf8_lossy(&network_view_bytes).contains("fetch("));
    assert!(html.contains("src=\"./graph-query.js\""));
    assert!(!String::from_utf8_lossy(&graph_query_bytes).contains("fetch("));
    assert!(!String::from_utf8_lossy(&stylesheet_bytes).contains("url(http"));

    let schema: Value = serde_json::from_slice(&schema_bytes).expect("schema JSON");
    assert_eq!(schema["additionalProperties"], false);
    let node_variants = schema["$defs"]["node"]["oneOf"]
        .as_array()
        .expect("node variants");
    assert_eq!(node_variants.len(), 2);
    for variant in node_variants {
        assert_eq!(variant["additionalProperties"], false);
    }
    let issue_node = node_variants
        .iter()
        .find(|variant| variant["properties"]["kind"]["const"] == "issue")
        .expect("Issue node variant");
    let external_node = node_variants
        .iter()
        .find(|variant| variant["properties"]["kind"]["const"] == "external_blocker")
        .expect("External blocker node variant");
    assert!(issue_node["properties"].get("url").is_some());
    assert!(external_node["properties"].get("url").is_none());
    assert_eq!(
        issue_node["properties"]["status"]["$ref"],
        "#/$defs/IssueNodeStatus"
    );
    assert_eq!(
        external_node["properties"]["status"]["$ref"],
        "#/$defs/ExternalNodeStatus"
    );
    assert!(
        node_variants
            .iter()
            .all(|variant| variant["properties"].get("common").is_some()
                && variant["properties"].get("state").is_none()
                && variant["properties"].get("readiness").is_none())
    );
    let scope_variants = schema["$defs"]["ScopeDescription"]["oneOf"]
        .as_array()
        .expect("execution-scope variants");
    assert_eq!(scope_variants.len(), 2);
    assert!(scope_variants.iter().all(|variant| {
        variant["additionalProperties"] == false
            && (variant["properties"]["mode"]["const"] != "available"
                || variant["properties"].get("assignee").is_none())
    }));
    assert_eq!(schema["$defs"]["edge"]["additionalProperties"], false);
    for (name, definition) in schema["$defs"].as_object().expect("schema definitions") {
        if definition.get("properties").is_some() {
            assert_eq!(
                definition["additionalProperties"], false,
                "schema definition {name} must be closed"
            );
        }
    }
    assert!(issue_node["properties"].get("projects").is_some());
    assert!(
        !issue_node["required"]
            .as_array()
            .expect("required node properties")
            .iter()
            .any(|field| field == "projects")
    );
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
        fs::read(output_directory.join("network-view.js"))
            .expect("regenerated network-view JavaScript"),
        network_view_bytes
    );
    assert_eq!(
        fs::read(output_directory.join("app.js")).expect("regenerated JavaScript"),
        javascript_bytes
    );
    assert_eq!(
        fs::read(output_directory.join("graph-query.js"))
            .expect("regenerated graph query JavaScript"),
        graph_query_bytes
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
            (2, blockers_for_two_with_external_state("unknown")),
            (3, "[]".to_owned()),
            (4, internal_blocker(5)),
            (5, internal_blocker(4)),
            (6, "[]".to_owned()),
            (7, internal_blocker(1)),
        ],
    );
    let generated = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("graph command");
    assert_success(&generated);
    mocks.assert();

    let constrained_output_directory = workspace.path().join("constrained-site");
    prepare_constrained_browser_site(&output_directory, &constrained_output_directory);
    let project_output_directory = workspace.path().join("project-site");
    prepare_project_browser_site(&output_directory, &project_output_directory);

    let result = run_browser_harness(
        &workspace,
        include_str!("fixtures/graph_browser_harness.js"),
    );
    assert!(result.get("error").is_none(), "browser result: {result}");
    let checks = result["checks"].as_object().expect("browser checks");
    let expected_checks = [
        "completed_summary_visible",
        "closed_nodes_honor_visual_encoding",
        "history_map_is_readable_and_not_unresolved",
        "constrained_completed_window",
        "completed_filter_and_details",
        "closed_empty_state",
        "completion_to_recommendation_preserves_analysis",
        "title_search",
        "number_search",
        "side_panel",
        "canonical_link",
        "precomputed_position",
        "scc_position",
        "labels_hidden_by_default",
        "selected_label_only",
        "keyboard_navigation",
        "layouts_use_precomputed_positions",
        "layout_and_camera_preserve_canonical_analysis",
        "camera_keyboard_pan",
        "camera_pointer_pan",
        "camera_anchored_wheel_zoom",
        "camera_touch_pinch_zoom",
        "camera_fit_shows_all_nodes",
        "expanded_map_keyboard_exit_and_inspection",
        "accessible_table_remains_visible",
        "zoom",
        "recommendation_summary",
        "distinct_runner_up",
        "operational_diagnostics",
        "causal_path",
        "unlock_size_control",
        "pagerank_size_control",
        "priority_color_control",
        "hostile_text_is_literal",
        "no_injected_elements",
        "full_network_within_validated_range",
        "performance_markers",
        "constrained_recommendation_preserves_metrics_and_evidence",
        "constrained_mode_opens_bounded",
        "constrained_search_keeps_table",
        "selected_result_opens_neighborhood",
        "constrained_reset_and_explicit_expand",
        "constrained_filters_keep_complete_table",
        "constrained_highlights_survive_window_changes",
        "constrained_isolation_and_clear",
        "readiness_filter",
        "state_filter",
        "priority_filter",
        "priority_conflict_filter",
        "area_filter",
        "assignee_filter",
        "assignee_named_unassigned_filter",
        "assignee_named_all_filter",
        "unassigned_filter",
        "filters_compose",
        "project_filter_absent_without_data",
        "project_filter_dom_interface",
        "disconnected_component_filter",
        "root_depth_isolation",
        "clear_restores_canonical_graph",
        "upstream_highlight",
        "downstream_highlight",
        "selected_path_highlight",
        "accessible_table_matches_visible_graph",
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
    assert_single_candidate_browser_reason();
}

fn assert_single_candidate_browser_reason() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site");
    let mocks = mock_repository(
        &mut github,
        json!([issue(1, "Only choice", "open")]).to_string(),
        vec![(1, "[]".to_owned())],
    );
    let generated = graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("single-candidate graph command");
    assert_success(&generated);
    mocks.assert();

    let result = run_browser_harness(
        &workspace,
        include_str!("fixtures/graph_single_candidate_harness.js"),
    );
    assert_eq!(result["canonical_reason"], true, "browser result: {result}");
}

fn run_browser_harness(workspace: &TempDir, script: &str) -> Value {
    let harness = workspace.path().join("browser-test.html");
    fs::write(
        &harness,
        "<!doctype html><html><head><style>iframe{width:1280px;height:900px}</style></head><body><iframe id=\"app\" src=\"./site/index.html\"></iframe><iframe id=\"constrained-app\" src=\"./constrained-site/index.html\"></iframe><iframe id=\"project-app\" src=\"./project-site/index.html\"></iframe><output id=\"result\">pending</output><script src=\"./browser-test.js\"></script></body></html>\n",
    )
    .expect("browser harness");
    fs::write(workspace.path().join("browser-test.js"), script)
        .expect("browser harness JavaScript");

    let browser_profile = TempDir::new().expect("temporary browser profile");
    let browser_binary = std::env::var_os("HYFA_BROWSER").unwrap_or_else(|| "google-chrome".into());
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
    browser_result(&String::from_utf8(browser.stdout).expect("browser DOM"))
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args(["graph", "--repo", "acme/widgets", "--output"]);
    command.arg(output);
    command.arg("--json");
    command
        .env("GH_TOKEN", "automation-token")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn offline_analysis_command(state: &TempDir, api_url: &str, command_name: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
    command.args([command_name, "--repo", "acme/widgets", "--json"]);
    command
        .env_remove("GH_TOKEN")
        .env("HYFA_GITHUB_API_URL", api_url)
        .env("HYFA_NO_KEYRING", "1")
        .env("HYFA_STATE_DIR", state.path())
        .env("PATH", "");
    command
}

fn analysis_projection(output: &Value) -> Value {
    let mut projection = output.as_object().expect("analysis output").clone();
    for envelope_field in [
        "schema_version",
        "policy_version",
        "command",
        "repository",
        "source",
        "synced_at",
        "replica_snapshot_hash",
        "execution_scope",
        "warnings",
    ] {
        projection.remove(envelope_field);
    }
    Value::Object(projection)
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
            "Root <script>alert(1)</script><!-- hyfa:operation operation-123 -->",
            "open"
        ),
        issue(2, "Dependent", "open"),
        issue(3, "Historical", "closed")
    ])
    .to_string()
}

fn browser_issue_inventory() -> String {
    let mut root = issue(
        1,
        "Root <script>alert(1)</script><!-- hyfa:operation operation-123 -->",
        "open",
    );
    root["labels"] = json!([
        label(9001, "area:<img src=x onerror=alert(2)>"),
        label(9101, "area:core"),
        label(9201, "priority:p1")
    ]);
    let mut dependent = issue(2, "Dependent", "open");
    dependent["assignees"] = json!([actor(2, "alice")]);
    dependent["labels"] = json!([label(9102, "area:web"), label(9202, "priority:p0")]);
    let mut historical = issue(3, "Historical", "closed");
    historical["labels"] = json!([label(9103, "area:docs")]);
    let mut cycle_a = issue(4, "Cycle A", "open");
    cycle_a["assignees"] = json!([actor(4, "alice")]);
    cycle_a["labels"] = json!([label(9104, "area:core"), label(9204, "priority:p2")]);
    let mut cycle_b = issue(5, "Cycle B", "open");
    cycle_b["assignees"] = json!([actor(5, "bob"), actor(55, "unassigned"), actor(56, "all")]);
    cycle_b["labels"] = json!([
        label(9105, "area:core"),
        label(9205, "priority:p2"),
        label(9305, "priority:p3")
    ]);
    json!([
        root,
        dependent,
        historical,
        cycle_a,
        cycle_b,
        issue(6, "Runner-up", "open"),
        issue(7, "Unlocked result", "open")
    ])
    .to_string()
}

fn prepare_project_browser_site(source: &std::path::Path, target: &std::path::Path) {
    fs::create_dir(target).expect("Project browser site directory");
    for asset in [
        "app.css",
        "graph-query.js",
        "graph-camera.js",
        "network-view.js",
        "app.js",
        "graph.schema.json",
    ] {
        fs::copy(source.join(asset), target.join(asset)).expect("copy Project browser site asset");
    }

    let mut graph: Value = serde_json::from_slice(
        &fs::read(source.join("graph.json")).expect("source graph artifact"),
    )
    .expect("source graph JSON");
    graph["nodes"][0]["projects"] = json!(["All"]);
    fs::write(
        target.join("graph.json"),
        serde_json::to_vec_pretty(&graph).expect("Project graph JSON"),
    )
    .expect("Project graph artifact");

    let mut html = fs::read_to_string(source.join("index.html")).expect("source graph HTML");
    let marker = "<script id=\"graph-data\" type=\"application/json\">";
    let data_start = html.find(marker).expect("embedded graph data") + marker.len();
    let data_end = data_start
        + html[data_start..]
            .find("</script>")
            .expect("embedded graph data terminator");
    let embedded = serde_json::to_string(&graph)
        .expect("embedded Project graph JSON")
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    html.replace_range(data_start..data_end, &embedded);
    fs::write(target.join("index.html"), html).expect("Project browser site HTML");
}

fn actor(id: u64, login: &str) -> Value {
    json!({"login": login, "id": id, "node_id": format!("U_{id}")})
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

fn prepare_constrained_browser_site(source: &std::path::Path, target: &std::path::Path) {
    fs::create_dir(target).expect("constrained browser site directory");
    for asset in [
        "app.css",
        "network-view.js",
        "graph-query.js",
        "graph-camera.js",
        "app.js",
        "graph.json",
        "graph.schema.json",
    ] {
        fs::copy(source.join(asset), target.join(asset)).expect("copy constrained site asset");
    }
    let mut html = fs::read_to_string(source.join("index.html")).expect("source graph HTML");
    let marker = "<script id=\"graph-presentation-data\" type=\"application/json\">";
    let data_start = html.find(marker).expect("embedded presentation data") + marker.len();
    let data_end = data_start
        + html[data_start..]
            .find("</script>")
            .expect("embedded presentation data terminator");
    let constrained = json!({
        "schema_version": "hyfa.graph-presentation/v1",
        "mode": "constrained",
        "full_network_limits": {"nodes": 0, "edges": 0},
        "initial_network_node_limit": 2,
        "initial_node_keys": ["acme/widgets#1"]
    });
    html.replace_range(data_start..data_end, &constrained.to_string());
    fs::write(target.join("index.html"), html).expect("constrained browser site HTML");
}

fn issue(number: u64, title: &str, state: &str) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": title,
        "body": "private body <!-- hyfa:operation operation-123 -->",
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

fn priority_issue(number: u64, title: &str, state: &str, priority: &str) -> Value {
    let mut issue = issue(number, title, state);
    issue["labels"]
        .as_array_mut()
        .expect("Issue labels")
        .push(json!({
            "id": 20000 + number,
            "node_id": format!("LP_{number}"),
            "name": priority,
            "color": "123456",
            "description": null
        }));
    issue
}

fn comment(id: u64, issue_number: u64) -> Value {
    json!({
        "id": id,
        "node_id": format!("IC_{id}"),
        "html_url": format!("https://github.com/acme/widgets/issues/{issue_number}#issuecomment-{id}"),
        "issue_url": format!("https://api.github.com/repos/acme/widgets/issues/{issue_number}"),
        "body": "private comment <!-- hyfa:operation operation-123 -->",
        "user": null,
        "author_association": "NONE",
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z"
    })
}

fn blockers_for_two() -> String {
    blockers_for_two_with_external_state("open")
}

fn blockers_for_two_with_external_state(state: &str) -> String {
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
            state
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
        .map(|node| node["common"]["key"].as_str().expect("node key"))
        .collect()
}

fn layer(graph: &Value, number: u64) -> Option<u64> {
    graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["common"]["number"] == number)
        .expect("node")
        .get("common")
        .and_then(|common| common.get("position"))
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
