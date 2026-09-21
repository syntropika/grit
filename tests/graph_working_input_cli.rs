use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use mockito::Server;
use serde_json::{Value, json};
use tempfile::TempDir;

#[allow(dead_code)]
mod support;

const REPOSITORY: &str = "acme/widgets";
const PRIVATE_TITLE: &str = "Pending private title";
const PRIVATE_BODY: &str = "Pending private Draft body";
const PRIVATE_COMMENT: &str = "Pending private comment";

#[test]
fn graph_next_and_plan_share_pending_drafts_dependencies_priorities_and_titles() {
    let fixture = Fixture::new();
    let synchronized_next = fixture.analysis("next", 3, None);
    let pending = fixture.queue_combined_changes();
    let outbox = fixture.outbox();
    let operation_ids = operation_ids(&outbox);
    let mut sorted_ids = operation_ids.clone();
    sorted_ids.sort();

    for horizon in [1, 2, 3] {
        for assignee in [None, Some("alice")] {
            let next = fixture.analysis("next", horizon, assignee);
            let plan = fixture.analysis("plan", horizon, assignee);
            let (_, graph) = fixture.private_graph(horizon, assignee);
            assert_eq!(next["pending"], true);
            assert_eq!(next["pending_operation_ids"], json!(operation_ids));
            assert_ne!(next["input_hash"], synchronized_next["input_hash"]);
            assert_analysis_parity(&next, &plan, &graph);
            assert_eq!(graph["provenance"]["state"], "pending");
            assert_eq!(
                graph["provenance"]["pending_operation_ids"],
                json!(sorted_ids)
            );
            assert_eq!(
                graph["provenance"]["pending_mutation_count"],
                operation_ids.len()
            );
            assert_eq!(graph["input_hash"], next["replica_snapshot_hash"]);
            assert_ordered_ranking_provenance(&next, &operation_ids);
            assert_no_synthetic_numbers(&next);
            assert_no_synthetic_numbers(&plan);
            assert_no_synthetic_numbers(&graph);

            let draft = graph_node(&graph, &pending.draft_key);
            assert_eq!(draft["title"], "Edited Draft title");
            assert_eq!(draft["priority"]["comparison"], "p0");
            assert!(draft["common"]["number"].is_null());
            assert_eq!(draft["common"]["temporary_id"], pending.temporary_id);
            assert_eq!(draft["common"]["provenance"]["state"], "pending");
            assert_eq!(draft["url"], "");
            assert_eq!(graph_node(&graph, "acme/widgets#1")["title"], PRIVATE_TITLE);
            assert_eq!(
                graph_node(&graph, "acme/widgets#1")["priority"]["comparison"],
                "p1"
            );
            assert!(graph["edges"].as_array().unwrap().iter().any(|edge| {
                edge["blocked"] == "acme/widgets#2"
                    && edge["blocker"] == pending.draft_key
                    && edge["provenance"]["state"] == "pending"
            }));
            let expected_first = if assignee.is_some() {
                "acme/widgets#1"
            } else {
                &pending.draft_key
            };
            assert_eq!(next["recommendation"]["first_issue"]["key"], expected_first);
            assert_eq!(
                plan["decision"]["recommendation"]["first_issue"]["key"],
                expected_first
            );
            let parallel = plan["parallel_now"].as_array().unwrap();
            assert!(parallel.iter().any(|issue| issue["key"] == expected_first));
            if assignee.is_none() {
                let draft = parallel
                    .iter()
                    .find(|issue| issue["key"] == pending.draft_key)
                    .unwrap();
                assert!(draft["number"].is_null());
                assert_eq!(draft["priority"]["comparison"], "p0");
            }
            assert_private_text_excluded(&graph);

            let restarted_next = fixture.analysis("next", horizon, assignee);
            let (_, restarted_graph) = fixture.private_graph(horizon, assignee);
            assert_eq!(next, restarted_next, "warm cache after a process restart");
            assert_eq!(
                graph, restarted_graph,
                "deterministic offline graph artifact"
            );
        }
    }
    fixture.assert_replica_unchanged();
    assert_eq!(
        fixture.outbox(),
        outbox,
        "analysis must not rewrite pending intent"
    );
}

#[test]
fn ordered_priority_intents_and_changed_content_cannot_reuse_stale_analysis() {
    let fixture = Fixture::new();
    let baseline = fixture.analysis("next", 3, None);
    assert_eq!(baseline["recommendation"]["first_issue"]["number"], 3);
    fixture.queue(&["update", "acme/widgets#1", "--priority", "p0", "--json"]);
    let promoted = fixture.analysis("next", 3, None);
    assert_eq!(promoted["recommendation"]["first_issue"]["number"], 1);

    fixture.queue(&["update", "acme/widgets#1", "--priority", "none", "--json"]);
    let cleared = fixture.analysis("next", 3, None);
    assert_eq!(cleared["recommendation"]["first_issue"]["number"], 3);
    assert_ne!(promoted["input_hash"], cleared["input_hash"]);

    let mut outbox = fixture.outbox();
    let ids_before = operation_ids(&outbox);
    assert_eq!(cleared["pending_operation_ids"], json!(ids_before));
    let last_operation = outbox["operations"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    last_operation["desired"] = json!({"state": "declared", "value": "p0"});
    fs::write(fixture.outbox_path(), serde_json::to_vec(&outbox).unwrap()).unwrap();

    let changed = fixture.analysis("next", 3, None);
    assert_eq!(operation_ids(&fixture.outbox()), ids_before);
    assert_eq!(changed["recommendation"]["first_issue"]["number"], 1);
    assert_ne!(changed["input_hash"], cleared["input_hash"]);
    let plan = fixture.analysis("plan", 3, None);
    let (_, graph) = fixture.private_graph(3, None);
    assert_analysis_parity(&changed, &plan, &graph);
    assert_eq!(changed, fixture.analysis("next", 3, None));
    fixture.assert_replica_unchanged();
}

#[test]
fn public_bundle_is_byte_identical_with_or_without_private_pending_operations() {
    let fixture = Fixture::new();
    let mut public_api = Server::new();
    let metadata = public_api
        .mock("GET", "/repos/acme/widgets")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "id": 999,
                "node_id": "R_widgets",
                "full_name": REPOSITORY,
                "html_url": "https://github.com/acme/widgets",
                "visibility": "public",
                "private": false,
                "owner": {"id": 888, "node_id": "O_acme", "login": "acme"}
            })
            .to_string(),
        )
        .expect(2)
        .create();
    // Inventory requests are unavailable, so both exports use the same stored snapshot.
    let before = fixture.public_bundle(&public_api.url());
    let pending = fixture.queue_combined_changes();
    let outbox = fixture.outbox();
    let after = fixture.public_bundle(&public_api.url());
    metadata.assert();
    assert_eq!(before, after);
    assert_eq!(
        before.keys().map(String::as_str).collect::<Vec<_>>(),
        ["graph.json", "graph.schema.json", "index.html"]
    );
    for bytes in after.values() {
        let text = String::from_utf8_lossy(bytes);
        for private in [
            PRIVATE_TITLE,
            PRIVATE_BODY,
            PRIVATE_COMMENT,
            &pending.draft_key,
        ] {
            assert!(
                !text.contains(private),
                "private Working input leaked: {private}"
            );
        }
        for id in operation_ids(&outbox) {
            assert!(
                !text.contains(&id),
                "pending provenance entered the public bundle"
            );
        }
    }
    fixture.assert_replica_unchanged();
    assert_eq!(fixture.outbox(), outbox);
}

struct Fixture {
    state: TempDir,
    unavailable: mockito::ServerGuard,
    synchronized: Vec<u8>,
}

struct PendingFixture {
    draft_key: String,
    temporary_id: Value,
}

impl Fixture {
    fn new() -> Self {
        let state = TempDir::new().unwrap();
        let mut github = Server::new();
        let mocks = support::mock_repository(
            &mut github,
            REPOSITORY,
            vec![
                support::issue(1, "open", &["priority:p4"], &[]),
                support::issue(2, "open", &["priority:p4"], &[]),
                support::issue(3, "open", &["priority:p1"], &[]),
            ],
            vec![(1, vec![]), (2, vec![]), (3, vec![])],
        );
        let synchronized = execute(
            command(&state, &github.url())
                .env("GH_TOKEN", "local-fixture-token")
                .args(["sync", "--repo", REPOSITORY, "--json"]),
        );
        assert_success(&synchronized);
        mocks.assert();
        let synchronized = fs::read(replica_path(&state)).unwrap();
        Self {
            state,
            unavailable: Server::new(),
            synchronized,
        }
    }

    fn queue(&self, args: &[&str]) -> Value {
        let output = execute(command(&self.state, &self.unavailable.url()).args(args));
        assert_success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn queue_combined_changes(&self) -> PendingFixture {
        self.queue(&["update", "acme/widgets#1", "--priority", "p1", "--json"]);
        self.queue(&[
            "update",
            "acme/widgets#1",
            "--title",
            PRIVATE_TITLE,
            "--json",
        ]);
        self.queue(&["update", "acme/widgets#1", "--assignee", "alice", "--json"]);
        let created = self.queue(&[
            "create",
            "--repo",
            REPOSITORY,
            "--title",
            "Private Draft",
            "--body",
            PRIVATE_BODY,
            "--json",
        ]);
        let draft_key = created["draft"]["key"].as_str().unwrap().to_owned();
        let temporary_id = created["draft"]["temporary_id"].clone();
        self.queue(&["update", &draft_key, "--priority", "p0", "--json"]);
        self.queue(&[
            "update",
            &draft_key,
            "--title",
            "Edited Draft title",
            "--json",
        ]);
        self.queue(&["block", "acme/widgets#2", "--by", &draft_key, "--json"]);
        self.queue(&[
            "comment",
            "acme/widgets#1",
            "--body",
            PRIVATE_COMMENT,
            "--json",
        ]);
        PendingFixture {
            draft_key,
            temporary_id,
        }
    }

    fn analysis(&self, name: &str, horizon: u8, assignee: Option<&str>) -> Value {
        let mut command = command(&self.state, &self.unavailable.url());
        command.args([name, "--repo", REPOSITORY, "--json", "--horizon"]);
        command.arg(horizon.to_string());
        if let Some(assignee) = assignee {
            command.args(["--assignee", assignee]);
        }
        let output = execute(&mut command);
        assert_success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn private_graph(&self, horizon: u8, assignee: Option<&str>) -> (TempDir, Value) {
        let directory = TempDir::new().unwrap();
        let mut command = command(&self.state, &self.unavailable.url());
        command.args(["graph", "--repo", REPOSITORY, "--output"]);
        command.arg(directory.path()).args(["--json", "--horizon"]);
        command.arg(horizon.to_string());
        if let Some(assignee) = assignee {
            command.args(["--assignee", assignee]);
        }
        let output = execute(&mut command);
        assert_success(&output);
        let graph = serde_json::from_slice(&fs::read(directory.path().join("graph.json")).unwrap())
            .unwrap();
        (directory, graph)
    }

    fn public_bundle(&self, api_url: &str) -> BTreeMap<String, Vec<u8>> {
        let directory = TempDir::new().unwrap();
        let output = execute(
            command(&self.state, api_url)
                .env("GH_TOKEN", "local-public-fixture-token")
                .args(["graph", "--repo", REPOSITORY, "--output"])
                .arg(directory.path())
                .args(["--public", "--json"]),
        );
        assert_success(&output);
        fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }

    fn outbox_path(&self) -> PathBuf {
        self.state
            .path()
            .join("repositories/acme/widgets/outbox.json")
    }

    fn outbox(&self) -> Value {
        serde_json::from_slice(&fs::read(self.outbox_path()).unwrap()).unwrap()
    }

    fn assert_replica_unchanged(&self) {
        assert_eq!(
            fs::read(replica_path(&self.state)).unwrap(),
            self.synchronized
        );
    }
}

fn command(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command
        .env("GRIT_STATE_DIR", state.path())
        .env("GRIT_GITHUB_API_URL", api_url)
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env("PATH", "");
    command
}

fn replica_path(state: &TempDir) -> PathBuf {
    state.path().join("repositories/acme/widgets/replica.json")
}

fn execute(command: &mut Command) -> Output {
    command.output().expect("execute isolated fixture command")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn operation_ids(outbox: &Value) -> Vec<String> {
    outbox["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|operation| operation["id"].as_str().unwrap().to_owned())
        .collect()
}

fn assert_analysis_parity(next: &Value, plan: &Value, graph: &Value) {
    assert_eq!(graph["effective_input_hash"], next["input_hash"]);
    assert_eq!(
        graph["effective_input_hash"],
        plan["decision"]["input_hash"]
    );
    let mut analysis = next.as_object().unwrap().clone();
    for field in [
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
        analysis.remove(field);
    }
    assert_eq!(graph["analysis"]["next"], Value::Object(analysis));
    for field in ["decision", "parallel_now", "dependency_layers"] {
        assert_eq!(graph["analysis"]["plan"][field], plan[field]);
    }
}

fn graph_node<'a>(graph: &'a Value, key: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["common"]["key"] == key)
        .expect("Working graph node")
}

fn assert_ordered_ranking_provenance(value: &Value, ordered_ids: &[String]) {
    match value {
        Value::Object(fields) => {
            if let Some(ids) = fields.get("operation_ids").and_then(Value::as_array) {
                let indices: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        ordered_ids
                            .iter()
                            .position(|candidate| id == candidate)
                            .expect("provenance must name a pending operation")
                    })
                    .collect();
                assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
            }
            for child in fields.values() {
                assert_ordered_ranking_provenance(child, ordered_ids);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_ordered_ranking_provenance(child, ordered_ids);
            }
        }
        _ => {}
    }
}

fn assert_no_synthetic_numbers(value: &Value) {
    match value {
        Value::Number(number) => assert!(
            number.as_u64().is_none_or(|number| number < (1_u64 << 63)),
            "internal Draft number leaked into serialized output: {number}"
        ),
        Value::Object(fields) => {
            for child in fields.values() {
                assert_no_synthetic_numbers(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_no_synthetic_numbers(child);
            }
        }
        _ => {}
    }
}

fn assert_private_text_excluded(graph: &Value) {
    let serialized = graph.to_string();
    for excluded in [
        PRIVATE_BODY,
        PRIVATE_COMMENT,
        "<!-- grit:operation",
        "<!-- grit-operation",
    ] {
        assert!(!serialized.contains(excluded));
    }
}
