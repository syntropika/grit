use std::{fs, process::Command};

use mockito::{Matcher, Mock, Server};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

mod support;

use support::browser::audit_local_page;

const PUBLIC_ARTIFACT_SCHEMA: &str = "grit.public-graph/v1";

#[test]
fn pages_workflow_has_least_privilege_and_uploads_the_sealed_contract() {
    let workflow_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/pages.yml");
    let workflow_source = fs::read_to_string(&workflow_path).expect("Pages workflow must exist");
    let documents =
        yaml_rust2::YamlLoader::load_from_str(&workflow_source).expect("valid Pages workflow YAML");
    let [workflow] = documents.as_slice() else {
        panic!("Pages workflow must contain one YAML document");
    };

    assert!(
        workflow["permissions"]
            .as_hash()
            .expect("top-level permissions map")
            .is_empty(),
        "workflow permissions must default to none"
    );
    let triggers = workflow["on"].as_hash().expect("structured triggers");
    assert_eq!(
        mapping_keys(triggers),
        ["push", "schedule", "workflow_dispatch"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(workflow["on"]["push"]["branches"][0].as_str(), Some("main"));
    assert!(
        workflow["on"]["schedule"][0]["cron"].as_str().is_some(),
        "scheduled refresh must have a cron expression"
    );
    assert_eq!(
        mapping_keys(workflow["jobs"].as_hash().expect("jobs map")),
        ["build", "deploy"].into_iter().map(str::to_owned).collect()
    );

    let build = &workflow["jobs"]["build"];
    assert_eq!(
        string_mapping(&build["permissions"]),
        [("contents", "read"), ("issues", "read")]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    );
    assert!(build["env"].is_badvalue());
    let build_steps = build["steps"].as_vec().expect("build steps");
    let graph_step = step_with_id(build_steps, "generate");
    assert_eq!(
        string_mapping(&graph_step["env"]),
        [
            ("GH_TOKEN", "${{ secrets.GITHUB_TOKEN }}"),
            ("GRIT_STATE_DIR", "${{ runner.temp }}/grit-state"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
    );
    let command = graph_step["run"]
        .as_str()
        .expect("public graph command")
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        command,
        "./target/release/grit graph --repo \"${{ github.repository }}\" --output site-public/ --public"
    );
    for step in build_steps {
        if step != graph_step {
            assert!(
                step["env"]["GH_TOKEN"].is_badvalue(),
                "GH_TOKEN leaked outside the graph generation step"
            );
        }
    }
    let archive = step_with_id(build_steps, "archive");
    assert_eq!(archive["env"]["INPUT_PATH"].as_str(), Some("site-public/"));
    assert_eq!(
        archive["run"]
            .as_str()
            .expect("archive command")
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        "tar --dereference --hard-dereference --directory \"$INPUT_PATH\" -cvf \"$RUNNER_TEMP/artifact.tar\" ."
    );
    let upload = step_with_id(build_steps, "upload");
    assert_action(upload, "actions/upload-artifact");
    assert_eq!(
        upload["with"]["path"].as_str(),
        Some("${{ runner.temp }}/artifact.tar")
    );
    assert_eq!(upload["with"]["name"].as_str(), Some("github-pages"));
    assert_action(step_with_id(build_steps, "checkout"), "actions/checkout");

    let deploy = &workflow["jobs"]["deploy"];
    assert_eq!(deploy["needs"].as_str(), Some("build"));
    assert_eq!(
        string_mapping(&deploy["permissions"]),
        [("id-token", "write"), ("pages", "write")]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    );
    assert!(deploy["env"].is_badvalue());
    assert_action(
        step_with_id(
            deploy["steps"].as_vec().expect("deploy steps"),
            "deployment",
        ),
        "actions/deploy-pages",
    );
    assert_eq!(
        workflow["concurrency"]["group"].as_str(),
        Some("grit-pages")
    );
    assert_eq!(
        workflow["concurrency"]["cancel-in-progress"].as_bool(),
        Some(true)
    );

    for step in build_steps
        .iter()
        .chain(deploy["steps"].as_vec().expect("deploy steps"))
    {
        if let Some(action) = step["uses"].as_str() {
            assert!(
                is_immutable_action_reference(action),
                "unpinned action: {action}"
            );
        }
    }

    let generated = generate_adversarial_public_site();
    assert_eq!(
        generated.files,
        ["graph.json", "graph.schema.json", "index.html"]
    );
    let schema = serde_json::to_string(&generated.schema).expect("serialized public schema");
    for artifact in [&generated.serialized, &schema, &generated.html] {
        assert!(
            !artifact.contains("automation-token"),
            "GitHub token entered the upload artifact"
        );
    }
}

#[test]
fn public_graph_fails_closed_before_refresh_when_repository_metadata_is_not_confirmed() {
    let cases = [
        (
            "private repository",
            repository_metadata_with(
                "acme/widgets",
                "https://github.com/acme/widgets",
                "private",
                true,
            ),
        ),
        (
            "internal repository",
            repository_metadata_with(
                "acme/widgets",
                "https://github.com/acme/widgets",
                "internal",
                false,
            ),
        ),
        (
            "unknown visibility",
            repository_metadata_with(
                "acme/widgets",
                "https://github.com/acme/widgets",
                "unknown",
                false,
            ),
        ),
        (
            "repository name mismatch",
            repository_metadata_with(
                "other/widgets",
                "https://github.com/other/widgets",
                "public",
                false,
            ),
        ),
        (
            "insecure URL",
            repository_metadata_with(
                "acme/widgets",
                "http://github.com/acme/widgets",
                "public",
                false,
            ),
        ),
        (
            "cross-repository URL",
            repository_metadata_with(
                "acme/widgets",
                "https://github.com/other/widgets",
                "public",
                false,
            ),
        ),
        (
            "credential-bearing URL",
            repository_metadata_with(
                "acme/widgets",
                "https://token@github.com/acme/widgets",
                "public",
                false,
            ),
        ),
        (
            "URL with query",
            repository_metadata_with(
                "acme/widgets",
                "https://github.com/acme/widgets?private=1",
                "public",
                false,
            ),
        ),
    ];
    for (case, body) in cases {
        let mut github = Server::new();
        let state = TempDir::new().expect("temporary state directory");
        let workspace = TempDir::new().expect("temporary graph workspace");
        let output_directory = workspace.path().join("site-public");
        fs::create_dir(&output_directory).expect("existing output directory");
        fs::write(
            output_directory.join("sentinel.txt"),
            "previous public site",
        )
        .expect("previous public site");
        let metadata = github
            .mock("GET", "/repos/acme/widgets")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .expect(1)
            .create();

        let output = public_graph_command(&state, &github.url(), &output_directory)
            .output()
            .expect("public graph command");

        assert!(!output.status.success(), "case: {case}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.to_ascii_lowercase().contains("public repository"),
            "case: {case}; stderr: {stderr}"
        );
        assert_eq!(
            fs::read_to_string(output_directory.join("sentinel.txt"))
                .expect("preserved public site"),
            "previous public site"
        );
        assert!(!output_directory.join("graph.json").exists());
        metadata.assert();
    }
}

#[test]
fn public_graph_refuses_stale_local_state_when_live_visibility_is_unavailable() {
    let mut github = Server::new();
    let api_url = github.url();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site-public");
    let mocks = mock_repository(
        &mut github,
        json!([issue(1, "Root", "open", 1, "private", "alice")]).to_string(),
        vec![(1, "[]".to_owned())],
        "private comment",
    );
    let synchronized = sync_command(&state, &api_url)
        .output()
        .expect("sync command");
    assert_success(&synchronized);
    mocks.assert();
    let visibility = github
        .mock("GET", "/repos/acme/widgets")
        .with_status(503)
        .expect(1)
        .create();

    let output = public_graph_command(&state, &api_url, &output_directory)
        .output()
        .expect("public graph command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("HTTP 503"), "stderr: {stderr}");
    assert!(!output_directory.exists());
    visibility.assert();
}

struct GeneratedPublicSite {
    command_output: Value,
    graph: Value,
    serialized: String,
    schema: Value,
    html: String,
    files: Vec<String>,
}

fn generate_adversarial_public_site() -> GeneratedPublicSite {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site-public");
    let metadata = mock_public_metadata(&mut github, 1);
    let issues = json!([
        issue(
            1,
            "Root fetch() ServiceWorker <script>alert('title')</script><!-- grit:operation operation-secret -->",
            "open",
            101,
            "root body secret",
            "alice"
        ),
        issue(
            2,
            "Externally blocked",
            "open",
            202,
            "blocked body secret",
            "bob"
        ),
        issue(
            3,
            "Closed history secret",
            "closed",
            303,
            "history body secret",
            "carol"
        ),
        issue(
            4,
            "Ready after closed history",
            "open",
            404,
            "ready body",
            "dora"
        ),
        issue(5, "Layer one", "open", 505, "layer body", "erin"),
        pull_request(6)
    ])
    .to_string();
    let dependencies = vec![
        (1, "[]".to_owned()),
        (
            2,
            blocker_list(&[
                internal_blocker(1, "open"),
                external_blocker("secret/private", 42, "open"),
                external_blocker("partners/internal", 77, "open"),
            ]),
        ),
        (3, "[]".to_owned()),
        (4, blocker_list(&[internal_blocker(3, "closed")])),
        (5, blocker_list(&[internal_blocker(1, "open")])),
    ];
    let mocks = mock_repository(&mut github, issues, dependencies, "private comment secret");
    let mut command = public_graph_command(&state, &github.url(), &output_directory);
    command.args(["--public-label-prefix", "area:"]);

    let output = command.output().expect("public graph command");
    assert_success(&output);
    let command_output: Value = serde_json::from_slice(&output.stdout).expect("command JSON");
    let bytes = fs::read(output_directory.join("graph.json")).expect("public graph artifact");
    let graph: Value = serde_json::from_slice(&bytes).expect("public graph JSON");
    let schema: Value = serde_json::from_slice(
        &fs::read(output_directory.join("graph.schema.json")).expect("public graph schema"),
    )
    .expect("schema JSON");
    let html = fs::read_to_string(output_directory.join("index.html")).expect("public HTML");
    let mut files = fs::read_dir(&output_directory)
        .expect("public bundle")
        .map(|entry| {
            entry
                .expect("public bundle entry")
                .file_name()
                .into_string()
                .expect("UTF-8 bundle name")
        })
        .collect::<Vec<_>>();
    files.sort();
    metadata.assert();
    mocks.assert();
    GeneratedPublicSite {
        command_output,
        graph,
        serialized: String::from_utf8(bytes).expect("UTF-8 public graph"),
        schema,
        html,
        files,
    }
}

#[test]
fn public_export_ignores_pending_drafts_fields_and_comments_after_live_visibility_check() {
    let mut github = Server::new();
    let state = TempDir::new().expect("state directory");
    let workspace = TempDir::new().expect("public workspace");
    let directory = workspace.path().join("site");
    let metadata = mock_public_metadata(&mut github, 2);
    let inventory = mock_repository(
        &mut github,
        json!([issue(
            1,
            "Synchronized title",
            "open",
            1,
            "private body",
            "alice"
        )])
        .to_string(),
        vec![(1, "[]".to_owned())],
        "private comment",
    );
    let seed = public_graph_command(&state, &github.url(), &directory)
        .output()
        .expect("seed public site");
    assert_success(&seed);
    inventory.assert();
    let original = fs::read(directory.join("graph.json")).expect("synchronized graph");
    for arguments in [
        vec![
            "create",
            "--repo",
            "acme/widgets",
            "--title",
            "Pending Draft secret",
            "--json",
        ],
        vec![
            "update",
            "acme/widgets#1",
            "--title",
            "Pending field secret",
            "--json",
        ],
        vec![
            "comment",
            "acme/widgets#1",
            "--body",
            "Pending comment secret",
            "--json",
        ],
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
        command.args(arguments);
        configure(&mut command, &state, &github.url());
        command.env_remove("GH_TOKEN");
        let queued = command.output().expect("queue local mutation");
        assert_success(&queued);
    }
    let unavailable = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(503)
        .create();
    let exported = public_graph_command(&state, &github.url(), &directory)
        .output()
        .expect("export synchronized fallback");
    assert_success(&exported);
    assert_eq!(
        fs::read(directory.join("graph.json")).expect("public graph"),
        original
    );
    for entry in fs::read_dir(&directory).expect("sealed bundle") {
        let contents =
            fs::read_to_string(entry.expect("bundle entry").path()).expect("bundle text");
        for secret in [
            "Pending Draft secret",
            "Pending field secret",
            "Pending comment secret",
        ] {
            assert!(!contents.contains(secret), "public export leaked {secret}");
        }
    }
    metadata.assert();
    unavailable.assert();
}

#[test]
fn public_bundle_has_a_closed_manifest_and_renders_hostile_text_literally() {
    let site = generate_adversarial_public_site();

    assert_eq!(
        site.files,
        ["graph.json", "graph.schema.json", "index.html"]
    );
    assert!(site.html.contains(
        "Root fetch() ServiceWorker &lt;script&gt;alert(&#39;title&#39;)&lt;/script&gt;"
    ));
    assert!(
        site.html
            .contains("area:backend&lt;img src=x onerror=alert(2)&gt;")
    );
    assert!(!site.html.contains("<script>alert('title')</script>"));
    assert!(!site.html.contains("<img src=x onerror=alert(2)>"));
}

#[test]
fn public_seal_rejects_a_secret_pattern_and_preserves_the_previous_bundle() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site-public");
    fs::create_dir(&output_directory).expect("existing output directory");
    fs::write(
        output_directory.join("sentinel.txt"),
        "previous sealed bundle",
    )
    .expect("previous bundle");
    let metadata = mock_public_metadata(&mut github, 1);
    let token = format!("github_pat_{}", "A".repeat(82));
    let mocks = mock_repository(
        &mut github,
        json!([issue(1, &token, "open", 1, "private", "alice")]).to_string(),
        vec![(1, "[]".to_owned())],
        "private comment",
    );

    let output = public_graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("public graph command");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("secret pattern"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(output_directory.join("sentinel.txt")).expect("previous bundle"),
        "previous sealed bundle"
    );
    assert!(!output_directory.join("graph.json").exists());
    metadata.assert();
    mocks.assert();
}

#[test]
#[ignore = "requires a Chrome-compatible browser; run explicitly as documented in README.md"]
fn sealed_public_bundle_executes_no_user_markup_or_external_request() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary public bundle workspace");
    let output_directory = workspace.path().join("site-public");
    let metadata = mock_public_metadata(&mut github, 1);
    let mocks = mock_repository(
        &mut github,
        json!([issue(
            1,
            "Root <script>alert('title')</script>",
            "open",
            1,
            "private body",
            "alice"
        )])
        .to_string(),
        vec![(1, "[]".to_owned())],
        "private comment",
    );
    let mut command = public_graph_command(&state, &github.url(), &output_directory);
    command.args(["--public-label-prefix", "area:"]);
    let generated = command.output().expect("public graph command");
    assert_success(&generated);
    metadata.assert();
    mocks.assert();

    let harness = workspace.path().join("browser-test.html");
    fs::write(
        &harness,
        "<!doctype html><html><body><iframe id=\"app\" src=\"./site-public/index.html\"></iframe><output id=\"result\">pending</output><script src=\"./browser-test.js\"></script></body></html>\n",
    )
    .expect("browser harness");
    fs::write(
        workspace.path().join("browser-test.js"),
        include_str!("fixtures/public_bundle_harness.js"),
    )
    .expect("browser harness JavaScript");

    let browser_binary = std::env::var_os("GRIT_BROWSER").unwrap_or_else(|| "google-chrome".into());
    let browser_profile = TempDir::new().expect("temporary browser profile");
    let audit = audit_local_page(&browser_binary, browser_profile.path(), &harness);
    let result = audit.result;
    assert_eq!(result["hostile_text_is_literal"], true, "{result}");
    assert_eq!(result["no_injected_elements"], true, "{result}");
    assert_eq!(result["resource_urls_are_local"], true, "{result}");
    assert!(
        audit.requests.iter().all(|url| url.starts_with("file://")),
        "public bundle attempted external network I/O: {:?}",
        audit.requests
    );
}

#[test]
#[ignore = "requires a Chrome-compatible browser; run explicitly as documented in README.md"]
fn browser_network_audit_detects_an_external_request_attempt() {
    let workspace = TempDir::new().expect("temporary browser workspace");
    let page = workspace.path().join("external-request.html");
    fs::write(
        &page,
        "<!doctype html><output id=\"result\">{\"ok\":true}</output><script>fetch('https://egress.invalid/private')</script>",
    )
    .expect("external request fixture");
    let profile = TempDir::new().expect("temporary browser profile");
    let browser_binary = std::env::var_os("GRIT_BROWSER").unwrap_or_else(|| "google-chrome".into());

    let audit = audit_local_page(&browser_binary, profile.path(), &page);

    assert!(
        audit
            .requests
            .iter()
            .any(|url| url == "https://egress.invalid/private"),
        "DevTools did not observe the external attempt: {:?}",
        audit.requests
    );
}

#[test]
fn public_projection_excludes_private_fields_and_anonymizes_external_blockers() {
    let site = generate_adversarial_public_site();
    let graph = &site.graph;

    assert_eq!(
        site.command_output["artifact"]["schema_version"],
        PUBLIC_ARTIFACT_SCHEMA
    );
    assert_eq!(site.command_output["artifact"]["node_count"], 4);
    assert_eq!(site.command_output["artifact"]["edge_count"], 2);
    assert_eq!(graph["schema_version"], PUBLIC_ARTIFACT_SCHEMA);
    assert_eq!(graph["repository"], "acme/widgets");
    assert_eq!(graph["visibility"], "public");
    assert_eq!(
        site.command_output["input_hash"],
        graph["public_input_hash"]
    );
    assert_eq!(
        node_keys(graph),
        vec![
            "acme/widgets#1",
            "acme/widgets#2",
            "acme/widgets#4",
            "acme/widgets#5"
        ]
    );
    assert_eq!(
        node(graph, 1)["url"],
        "https://github.com/acme/widgets/issues/1"
    );
    assert_eq!(
        node(graph, 1)["title"],
        "Root fetch() ServiceWorker <script>alert('title')</script>"
    );
    assert_eq!(
        node(graph, 1)["labels"],
        json!(["area:backend<img src=x onerror=alert(2)>"])
    );
    for public_node in graph["nodes"].as_array().expect("nodes") {
        assert!(public_node.get("assignees").is_none());
        assert!(public_node.get("projects").is_none());
        assert_eq!(
            public_node["key"]
                .as_str()
                .expect("node key")
                .split('#')
                .next(),
            Some("acme/widgets")
        );
    }
    for prohibited in [
        "secret/private",
        "partners/internal",
        "#42",
        "#77",
        "private comment",
        "body secret",
        "Closed history secret",
        "operation-secret",
        "label-operation",
        "secret:customer",
        "risk:high",
        "alice",
        "I_101",
        "L_secret",
        "https://evil.example",
    ] {
        assert!(!site.serialized.contains(prohibited), "leaked {prohibited}");
    }
}

#[test]
fn public_projection_recomputes_readiness_edges_and_dependency_layers() {
    let site = generate_adversarial_public_site();
    let graph = &site.graph;

    assert_eq!(graph["operational_counts"]["issue_count"], 4);
    assert_eq!(graph["operational_counts"]["ready_count"], 2);
    assert_eq!(graph["operational_counts"]["blocked_count"], 2);
    assert_eq!(node(graph, 1)["readiness"], "ready");
    assert_eq!(node(graph, 1)["position"]["layer"], 0);
    assert_eq!(node(graph, 2)["readiness"], "external_unknown");
    assert_eq!(node(graph, 2)["position"]["layer"], Value::Null);
    assert_eq!(node(graph, 4)["readiness"], "ready");
    assert_eq!(node(graph, 4)["position"]["layer"], 0);
    assert_eq!(node(graph, 5)["readiness"], "blocked");
    assert_eq!(node(graph, 5)["position"]["layer"], 1);
    assert_eq!(
        graph["edges"],
        json!([
            {"blocked": "acme/widgets#2", "blocker": "acme/widgets#1", "kind": "blocked_by"},
            {"blocked": "acme/widgets#5", "blocker": "acme/widgets#1", "kind": "blocked_by"}
        ])
    );
    assert_all_edge_endpoints_exist(graph);
}

#[test]
fn public_schema_urls_and_html_are_closed_and_repository_scoped() {
    let site = generate_adversarial_public_site();

    assert_closed_schema(&site.schema);
    assert_eq!(
        site.schema["$defs"]["public_node"]["properties"]["url"]["pattern"],
        r"^https://github\.com/acme/widgets/issues/[1-9][0-9]*$"
    );
    assert!(site.html.contains("acme/widgets#2"));
    assert!(!site.html.contains("Assignees"));
    assert!(!site.html.contains("Projects"));
    assert!(!site.html.contains("secret/private"));
    assert!(!site.html.contains("private comment"));
}

#[test]
fn assignees_are_published_only_after_explicit_public_opt_in() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site-public");
    let metadata = mock_public_metadata(&mut github, 1);
    let mocks = mock_repository(
        &mut github,
        json!([issue(1, "Root", "open", 1, "private body", "alice")]).to_string(),
        vec![(1, "[]".to_owned())],
        "private comment",
    );
    let mut command = public_graph_command(&state, &github.url(), &output_directory);
    command.arg("--public-include-assignees");

    let output = command.output().expect("public graph command");
    assert_success(&output);
    let graph: Value = serde_json::from_slice(
        &fs::read(output_directory.join("graph.json")).expect("public graph artifact"),
    )
    .expect("public graph JSON");
    assert_eq!(node(&graph, 1)["assignees"], json!(["alice"]));
    assert!(node(&graph, 1).get("labels").is_none());
    let html = fs::read_to_string(output_directory.join("index.html")).expect("public HTML");
    assert!(html.contains("Assignees"));
    assert!(html.contains("alice"));
    assert!(!html.contains("Labels"));
    metadata.assert();
    mocks.assert();
}

#[test]
fn hidden_data_and_external_identity_cannot_change_public_analysis() {
    let first = public_projection(hidden_variant(false));
    let second = public_projection(hidden_variant(true));

    assert_eq!(first["public_input_hash"], second["public_input_hash"]);
    assert_eq!(first["artifact_hash"], second["artifact_hash"]);
    assert_eq!(first["operational_counts"], second["operational_counts"]);
    assert_eq!(first["nodes"], second["nodes"]);
    assert_eq!(first["edges"], second["edges"]);
}

fn public_projection(variant: HiddenVariant) -> Value {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let workspace = TempDir::new().expect("temporary graph workspace");
    let output_directory = workspace.path().join("site-public");
    let metadata = mock_public_metadata(&mut github, 1);
    let issues = json!([
        issue(
            1,
            "Root",
            "open",
            variant.root_id,
            &variant.body,
            &variant.assignee
        ),
        issue(
            2,
            "Opaque boundary",
            "open",
            variant.blocked_id,
            "body",
            "owner"
        ),
        issue(
            variant.closed_number,
            &variant.closed_title,
            "closed",
            variant.closed_id,
            "history",
            "historian"
        )
    ])
    .to_string();
    let dependencies = vec![
        (1, "[]".to_owned()),
        (2, blocker_list(&variant.external_blockers)),
        (variant.closed_number, "[]".to_owned()),
    ];
    let mocks = mock_repository(&mut github, issues, dependencies, &variant.comment);

    let output = public_graph_command(&state, &github.url(), &output_directory)
        .output()
        .expect("public graph command");
    assert_success(&output);
    let graph = serde_json::from_slice(
        &fs::read(output_directory.join("graph.json")).expect("public graph artifact"),
    )
    .expect("public graph JSON");
    metadata.assert();
    mocks.assert();
    graph
}

struct HiddenVariant {
    root_id: u64,
    blocked_id: u64,
    body: String,
    assignee: String,
    comment: String,
    closed_number: u64,
    closed_id: u64,
    closed_title: String,
    external_blockers: Vec<Value>,
}

fn hidden_variant(second: bool) -> HiddenVariant {
    if second {
        HiddenVariant {
            root_id: 9001,
            blocked_id: 9002,
            body: "different private body".to_owned(),
            assignee: "different-owner".to_owned(),
            comment: "different private comment".to_owned(),
            closed_number: 99,
            closed_id: 9099,
            closed_title: "Different closed history".to_owned(),
            external_blockers: vec![
                external_blocker("different/private", 700, "open"),
                external_blocker("another/internal", 701, "open"),
            ],
        }
    } else {
        HiddenVariant {
            root_id: 101,
            blocked_id: 102,
            body: "private body".to_owned(),
            assignee: "alice".to_owned(),
            comment: "private comment".to_owned(),
            closed_number: 3,
            closed_id: 103,
            closed_title: "Closed history".to_owned(),
            external_blockers: vec![external_blocker("secret/private", 42, "open")],
        }
    }
}

fn public_graph_command(state: &TempDir, api_url: &str, output: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["graph", "--repo", "acme/widgets", "--output"]);
    command.arg(output);
    command.args(["--public", "--json"]);
    configure(&mut command, state, api_url);
    command
}

fn sync_command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["sync", "--repo", "acme/widgets", "--json"]);
    configure(&mut command, state, api_url);
    command
}

fn configure(command: &mut Command, state: &TempDir, api_url: &str) {
    command
        .env("GH_TOKEN", "automation-token")
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
}

fn mock_public_metadata(github: &mut Server, expected: usize) -> Mock {
    github
        .mock("GET", "/repos/acme/widgets")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(repository_metadata("public", false))
        .expect(expected)
        .create()
}

fn repository_metadata(visibility: &str, private: bool) -> String {
    repository_metadata_with(
        "acme/widgets",
        "https://github.com/acme/widgets",
        visibility,
        private,
    )
}

fn repository_metadata_with(
    full_name: &str,
    html_url: &str,
    visibility: &str,
    private: bool,
) -> String {
    json!({
        "id": 999,
        "node_id": "R_secret",
        "full_name": full_name,
        "html_url": html_url,
        "visibility": visibility,
        "private": private,
        "owner": {"id": 888, "node_id": "O_secret", "login": "acme"}
    })
    .to_string()
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
    comment_body: &str,
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
        .expect(1)
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!([comment(501, 1, comment_body)]).to_string())
        .expect(1)
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
                .expect(1)
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

fn issue(number: u64, title: &str, state: &str, id: u64, body: &str, assignee: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("I_{id}"),
        "number": number,
        "title": title,
        "body": body,
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://evil.example/stolen/{number}"),
        "user": {"id": 8000 + number, "node_id": format!("U_{number}"), "login": "author"},
        "assignees": [{"id": 9000 + number, "node_id": format!("A_{number}"), "login": assignee}],
        "labels": [
            {"id": 10000 + number, "node_id": format!("L_area_{number}"), "name": "area:backend<img src=x onerror=alert(2)><!-- grit:operation label-operation -->", "color": "123456", "description": null},
            {"id": 11000 + number, "node_id": "L_secret", "name": "secret:customer", "color": "654321", "description": "private"},
            {"id": 12000 + number, "node_id": format!("L_risk_{number}"), "name": "risk:high", "color": "abcdef", "description": null},
            {"id": 13000 + number, "node_id": format!("L_ready_{number}"), "name": "ready", "color": "abcdef", "description": null},
            {"id": 14000 + number, "node_id": format!("L_open_{number}"), "name": "open", "color": "abcdef", "description": null},
            {"id": 15000 + number, "node_id": format!("L_repository_{number}"), "name": "repository", "color": "abcdef", "description": null},
            {"id": 16000 + number, "node_id": format!("L_unresolved_{number}"), "name": "unresolved", "color": "abcdef", "description": null}
        ],
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": if state == "closed" { Some("2026-08-02T00:00:00Z") } else { None }
    })
}

fn pull_request(number: u64) -> Value {
    let mut value = issue(
        number,
        "Pull request",
        "open",
        number * 100,
        "PR body",
        "reviewer",
    );
    value["pull_request"] = json!({"url": "https://api.github.com/repos/acme/widgets/pulls/6"});
    value
}

fn comment(id: u64, issue_number: u64, body: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("IC_{id}"),
        "html_url": format!("https://github.com/acme/widgets/issues/{issue_number}#issuecomment-{id}"),
        "issue_url": format!("https://api.github.com/repos/acme/widgets/issues/{issue_number}"),
        "body": body,
        "user": {"id": 7, "node_id": "U_secret", "login": "commenter"},
        "author_association": "MEMBER",
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z"
    })
}

fn blocker_list(blockers: &[Value]) -> String {
    Value::Array(blockers.to_vec()).to_string()
}

fn internal_blocker(number: u64, state: &str) -> Value {
    blocker("acme/widgets", number, state)
}

fn external_blocker(repository: &str, number: u64, state: &str) -> Value {
    blocker(repository, number, state)
}

fn blocker(repository: &str, number: u64, state: &str) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("B_{number}"),
        "repository_url": format!("https://api.github.com/repos/{repository}"),
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

fn node(graph: &Value, number: u64) -> &Map<String, Value> {
    graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["number"] == number)
        .and_then(Value::as_object)
        .expect("node")
}

fn assert_all_edge_endpoints_exist(graph: &Value) {
    let keys = node_keys(graph);
    for edge in graph["edges"].as_array().expect("edges") {
        assert!(keys.contains(&edge["blocked"].as_str().expect("blocked key")));
        assert!(keys.contains(&edge["blocker"].as_str().expect("blocker key")));
    }
}

fn assert_closed_schema(schema: &Value) {
    assert_eq!(schema["additionalProperties"], false);
    for definition in schema["$defs"]
        .as_object()
        .expect("schema definitions")
        .values()
    {
        if definition.get("properties").is_some() {
            assert_eq!(
                definition["additionalProperties"], false,
                "definition: {definition}"
            );
        }
    }
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mapping_keys(mapping: &yaml_rust2::yaml::Hash) -> std::collections::BTreeSet<String> {
    mapping
        .keys()
        .map(|key| key.as_str().expect("string mapping key").to_owned())
        .collect()
}

fn string_mapping(value: &yaml_rust2::Yaml) -> std::collections::BTreeMap<String, String> {
    value
        .as_hash()
        .expect("string mapping")
        .iter()
        .map(|(key, value)| {
            (
                key.as_str().expect("string mapping key").to_owned(),
                value.as_str().expect("string mapping value").to_owned(),
            )
        })
        .collect()
}

fn step_with_id<'a>(steps: &'a [yaml_rust2::Yaml], id: &str) -> &'a yaml_rust2::Yaml {
    steps
        .iter()
        .find(|step| step["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing workflow step {id}"))
}

fn is_immutable_action_reference(reference: &str) -> bool {
    let Some((_, revision)) = reference.rsplit_once('@') else {
        return false;
    };
    revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn assert_action(step: &yaml_rust2::Yaml, expected: &str) {
    let action = step["uses"].as_str().expect("action step");
    assert_eq!(action.split_once('@').map(|(name, _)| name), Some(expected));
}
