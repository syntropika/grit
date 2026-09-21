use std::{fs, process::Command};

use mockito::Server;
use serde_json::{Value, json};
use tempfile::TempDir;

mod support;

use support::{
    assert_success, external_blocker, internal_blocker, internal_blocker_for, issue, issue_for,
    mock_repository,
};

#[test]
fn plan_rejects_worker_capacity_in_v1_before_accessing_github() {
    let output = Command::new(env!("CARGO_BIN_EXE_grit"))
        .args(["plan", "--repo", "acme/widgets", "--workers", "2", "--json"])
        .env("GRIT_GITHUB_API_URL", "http://127.0.0.1:1")
        .env("GH_TOKEN", "test-token")
        .output()
        .expect("run grit plan with worker capacity");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("grit plan does not accept --workers in v1"));
    assert!(!stderr.contains("GitHub refresh failed"));
}

#[test]
fn plan_reuses_next_decision_and_ready_execution_frontier() {
    let mut github = Server::new();
    let mocks = mock_repository(
        &mut github,
        "acme/plan",
        vec![
            issue(1, "open", &["priority:p1"], &[]),
            issue(2, "open", &["priority:p2"], &[]),
            issue(3, "open", &[], &["alice"]),
        ],
        vec![
            (1, vec![]),
            (2, vec![]),
            (3, vec![internal_blocker(1, "open")]),
        ],
    );
    let state = TempDir::new().expect("temporary state directory");
    let api_url = github.url();

    let seed = grit_command(&state, &api_url, "ready")
        .output()
        .expect("seed Local replica");
    assert_success(&seed);
    mocks.assert();
    drop(github);

    let plan = grit_command(&state, &api_url, "plan")
        .output()
        .expect("offline grit plan");
    let next = grit_command(&state, &api_url, "next")
        .arg("--profile")
        .output()
        .expect("offline grit next");
    let ready = grit_command(&state, &api_url, "ready")
        .output()
        .expect("offline grit ready");
    assert_success(&next);
    assert_success(&ready);
    assert_success(&plan);

    let next: Value = serde_json::from_slice(&next.stdout).expect("next JSON");
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("ready JSON");
    let plan: Value = serde_json::from_slice(&plan.stdout).expect("plan JSON");
    assert_eq!(plan["schema_version"], "grit.plan/v1");
    assert_eq!(plan["policy_version"], "next/v1");
    assert_eq!(plan["command"], "plan");
    assert_eq!(next["performance"]["cache_hit"], true);
    for field in [
        "input_hash",
        "pending",
        "pending_operation_ids",
        "mode",
        "metrics",
        "recommendation",
        "comparison_to_runner_up",
        "close_call",
        "search_complete",
        "truncated_by",
        "global_optimum_claimed",
        "runner_up_scope",
        "summary",
        "work",
    ] {
        assert_eq!(plan["decision"][field], next[field], "shared {field}");
    }
    for field in ["horizon", "state_budget", "pagerank"] {
        assert_eq!(
            plan["decision"]["parameters"][field], next["parameters"][field],
            "shared parameter {field}"
        );
    }
    assert!(plan["decision"].get("alternatives").is_none());
    assert!(
        plan["decision"]["parameters"]
            .get("alternative_limit")
            .is_none()
    );
    assert_eq!(
        plan["decision"]["recommendation"]["first_issue"]["number"],
        1
    );
    assert_eq!(
        issue_numbers(&plan["parallel_now"]),
        issue_numbers(&ready["issues"])
    );
    assert_eq!(issue_numbers(&plan["parallel_now"]), vec![1, 2]);
}

#[test]
fn pending_plan_and_next_share_cached_decisions_and_projected_structural_priorities() {
    let repository = "acme/pending-plan";
    let issues = vec![
        issue_for(repository, 1, "open", &["priority:p4"], &[]),
        issue_for(repository, 2, "open", &["priority:p1"], &[]),
        issue_for(repository, 3, "open", &["priority:p4"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![internal_blocker_for(repository, 1, "open")]),
    ];
    let (state, api_url) = seeded_replica(repository, issues, dependencies);
    let baseline = grit_command_for(&state, &api_url, repository, "next", None)
        .output()
        .expect("rank synchronized priorities");
    assert_success(&baseline);
    let baseline: Value = serde_json::from_slice(&baseline.stdout).expect("baseline next JSON");
    let repository_directory = state.path().join("repositories/acme/pending-plan");
    let snapshot_path = repository_directory.join("replica.json");
    let snapshot_before = fs::read(&snapshot_path).expect("synchronized snapshot");
    let mut operation_ids = Vec::new();
    for (number, priority) in [(3, "p0"), (2, "p2")] {
        let output = Command::new(env!("CARGO_BIN_EXE_grit"))
            .args([
                "update",
                &format!("{repository}#{number}"),
                "--priority",
                priority,
                "--json",
            ])
            .env("GRIT_NO_KEYRING", "1")
            .env("GRIT_STATE_DIR", state.path())
            .env("GRIT_GITHUB_API_URL", &api_url)
            .env("GH_TOKEN", "test-token")
            .output()
            .expect("queue pending Priority");
        assert_success(&output);
        let output: Value = serde_json::from_slice(&output.stdout).expect("pending update JSON");
        operation_ids.push(output["operation"]["id"].clone());
    }
    let outbox_path = repository_directory.join("outbox.json");
    let outbox_before = fs::read(&outbox_path).expect("pending mutations");
    let plan_command = || grit_command_for(&state, &api_url, repository, "plan", None);
    let plan = plan_command().output().expect("plan pending graph");
    let next = grit_command_for(&state, &api_url, repository, "next", None)
        .arg("--profile")
        .output()
        .expect("reuse pending plan cache in next");
    let repeated_plan = plan_command().output().expect("repeat cached pending plan");
    assert_success(&plan);
    assert_success(&next);
    assert_success(&repeated_plan);
    assert_eq!(plan.stdout, repeated_plan.stdout);
    let plan: Value = serde_json::from_slice(&plan.stdout).expect("pending plan JSON");
    let next: Value = serde_json::from_slice(&next.stdout).expect("pending next JSON");
    assert_eq!(next["performance"]["cache_hit"], true);
    for (field, value) in plan["decision"].as_object().expect("decision object") {
        if field == "parameters" {
            for (parameter, expected) in value.as_object().expect("parameters") {
                assert_eq!(expected, &next[field][parameter], "shared {parameter}");
            }
        } else {
            assert_eq!(value, &next[field], "shared {field}");
        }
    }
    assert_ne!(next["input_hash"], baseline["input_hash"]);
    assert_eq!(plan["decision"]["pending"], true);
    assert_eq!(
        plan["decision"]["pending_operation_ids"],
        json!(operation_ids)
    );
    assert_eq!(plan["decision"]["mode"], "p0_route");
    assert_eq!(
        plan["decision"]["recommendation"]["first_issue"]["number"],
        1
    );
    assert_eq!(
        plan["decision"]["recommendation"]["operation_ids"],
        json!(operation_ids)
    );
    assert_eq!(layer_numbers(&plan), vec![vec![1, 2], vec![3]]);
    let target = &layer(&plan, 1)["issues"][0];
    assert_eq!(target["priority"]["value"], "p0");
    assert_eq!(target["operation_ids"], json!([operation_ids[0]]));
    let parallel = plan["parallel_now"]
        .as_array()
        .expect("parallel Issues")
        .iter()
        .find(|issue| issue["number"] == 2)
        .expect("pending parallel Issue");
    assert_eq!(parallel["priority"]["value"], "p2");
    assert_eq!(parallel["operation_ids"], json!([operation_ids[1]]));
    assert_eq!(
        fs::read(snapshot_path).expect("unchanged snapshot"),
        snapshot_before
    );
    assert_eq!(
        fs::read(outbox_path).expect("unchanged pending mutations"),
        outbox_before
    );
}

#[test]
fn pending_dependency_chain_and_removal_rebuild_the_cached_plan_and_next_graph() {
    let repository = "acme/pending-topology";
    let (state, api_url) = seeded_replica(
        repository,
        vec![
            issue_for(repository, 1, "open", &[], &[]),
            issue_for(repository, 2, "open", &[], &[]),
            issue_for(repository, 3, "open", &["priority:p0"], &[]),
        ],
        vec![(1, vec![]), (2, vec![]), (3, vec![])],
    );
    let ranked = |command| {
        let mut invocation = grit_command_for(&state, &api_url, repository, command, None);
        if command == "next" {
            invocation.arg("--profile");
        }
        let output = invocation
            .output()
            .expect("analyze projected dependency graph");
        assert_success(&output);
        serde_json::from_slice::<Value>(&output.stdout).expect("analysis JSON")
    };
    let baseline = ranked("next");
    assert_eq!(baseline["mode"], "p0_ready");
    let directory = state.path().join("repositories/acme/pending-topology");
    let replica_before = fs::read(directory.join("replica.json")).expect("base replica");
    let mut operation_ids = Vec::new();
    let mut previous_hash = baseline["input_hash"].clone();
    for (command, blocked, blocker, expected_steps, expected_layers) in [
        ("block", 3, 2, vec![2, 3, 1], vec![vec![1, 2], vec![3]]),
        (
            "block",
            2,
            1,
            vec![1, 2, 3],
            vec![vec![1], vec![2], vec![3]],
        ),
        ("unblock", 2, 1, vec![2, 3, 1], vec![vec![1, 2], vec![3]]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_grit"))
            .args([
                command,
                &format!("{repository}#{blocked}"),
                "--by",
                &format!("{repository}#{blocker}"),
                "--json",
            ])
            .env("GRIT_NO_KEYRING", "1")
            .env("GRIT_STATE_DIR", state.path())
            .env("GRIT_GITHUB_API_URL", &api_url)
            .env("GH_TOKEN", "test-token")
            .output()
            .expect("queue Dependency mutation");
        assert_success(&output);
        let output: Value = serde_json::from_slice(&output.stdout).expect("pending mutation");
        operation_ids.push(output["operation"]["id"].clone());
        let pending_before = fs::read(directory.join("outbox.json")).expect("pending outbox");
        let next = ranked("next");
        let plan = ranked("plan");
        let warm = ranked("next");
        assert_eq!(next["performance"]["cache_hit"], false);
        assert_eq!(warm["performance"]["cache_hit"], true);
        assert_ne!(next["input_hash"], previous_hash);
        previous_hash = next["input_hash"].clone();
        assert_eq!(next["mode"], "p0_route");
        assert_eq!(next["pending_operation_ids"], json!(operation_ids));
        assert_eq!(
            next["recommendation"]["operation_ids"],
            json!(operation_ids)
        );
        let steps = next["recommendation"]["rollout"]["steps"]
            .as_array()
            .expect("rollout");
        assert_eq!(
            steps
                .iter()
                .map(|step| step["issue"]["number"].as_u64().expect("Issue number"))
                .collect::<Vec<_>>(),
            expected_steps
        );
        assert_eq!(layer_numbers(&plan), expected_layers);
        assert_eq!(plan["parallel_now"][0]["pending"], true);
        assert_eq!(
            plan["parallel_now"][0]["operation_ids"],
            json!(operation_ids)
        );
        for (field, value) in plan["decision"].as_object().expect("decision") {
            if field == "parameters" {
                for (parameter, expected) in value.as_object().expect("parameters") {
                    assert_eq!(expected, &next[field][parameter]);
                }
            } else {
                assert_eq!(value, &next[field], "shared {field}");
                assert_eq!(value, &warm[field], "cached shared {field}");
            }
        }
        assert_eq!(
            fs::read(directory.join("outbox.json")).expect("outbox after reads"),
            pending_before
        );
        assert_eq!(
            fs::read(directory.join("replica.json")).expect("replica after reads"),
            replica_before
        );
    }
}

#[test]
fn dependency_layers_use_and_depth_and_annotate_the_full_graph_scope() {
    let repository = "acme/layers";
    let issues = vec![
        issue_for(repository, 1, "open", &[], &[]),
        issue_for(repository, 2, "open", &[], &["alice"]),
        issue_for(repository, 3, "open", &[], &[]),
        issue_for(repository, 4, "open", &[], &[]),
        issue_for(repository, 5, "closed", &[], &[]),
        issue_for(repository, 6, "open", &[], &[]),
        issue_for(repository, 7, "open", &[], &["bob"]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![internal_blocker_for(repository, 1, "open")]),
        (
            4,
            vec![
                internal_blocker_for(repository, 1, "open"),
                internal_blocker_for(repository, 3, "open"),
            ],
        ),
        (5, vec![]),
        (6, vec![internal_blocker_for(repository, 5, "closed")]),
        (7, vec![]),
    ];
    let (state, api_url) = seeded_replica(repository, issues, dependencies);

    let default = grit_command_for(&state, &api_url, repository, "plan", None)
        .output()
        .expect("default plan");
    let alice = grit_command_for(&state, &api_url, repository, "plan", Some("alice"))
        .output()
        .expect("alice plan");
    let alice_ready = grit_command_for(&state, &api_url, repository, "ready", Some("alice"))
        .output()
        .expect("alice ready");
    assert_success(&default);
    assert_success(&alice);
    assert_success(&alice_ready);
    let default: Value = serde_json::from_slice(&default.stdout).expect("default plan JSON");
    let alice: Value = serde_json::from_slice(&alice.stdout).expect("alice plan JSON");
    let alice_ready: Value = serde_json::from_slice(&alice_ready.stdout).expect("alice ready JSON");

    assert_eq!(
        layer_numbers(&default),
        vec![vec![1, 2, 6, 7], vec![3], vec![4]]
    );
    assert_eq!(layer_numbers(&alice), layer_numbers(&default));
    assert_eq!(unresolved_numbers(&default), Vec::<u64>::new());
    assert_eq!(
        issue_numbers(&alice["parallel_now"]),
        issue_numbers(&alice_ready["issues"])
    );
    assert_eq!(issue_numbers(&alice["parallel_now"]), vec![2]);

    let default_layer_zero = layer(&default, 0);
    assert!(!issue_annotation(default_layer_zero, 1, "assigned"));
    assert!(issue_annotation(default_layer_zero, 1, "executable"));
    assert!(issue_annotation(
        default_layer_zero,
        1,
        "execution_scope_eligible"
    ));
    assert!(issue_annotation(default_layer_zero, 2, "assigned"));
    assert!(!issue_annotation(default_layer_zero, 2, "executable"));
    assert!(!issue_annotation(
        default_layer_zero,
        2,
        "execution_scope_eligible"
    ));
    assert!(!issue_annotation(default_layer_zero, 7, "executable"));
    let default_layer_one = layer(&default, 1);
    assert!(!issue_annotation(default_layer_one, 3, "ready_now"));
    assert!(!issue_annotation(default_layer_one, 3, "executable"));
    assert!(issue_annotation(
        default_layer_one,
        3,
        "execution_scope_eligible"
    ));
    let alice_layer_zero = layer(&alice, 0);
    assert!(!issue_annotation(alice_layer_zero, 1, "executable"));
    assert!(issue_annotation(alice_layer_zero, 2, "executable"));
    assert!(!issue_annotation(alice_layer_zero, 7, "executable"));
    assert!(!issue_annotation(
        layer(&alice, 1),
        3,
        "execution_scope_eligible"
    ));
}

#[test]
fn dependency_layers_propagate_every_unresolved_boundary() {
    let repository = "acme/unresolved";
    let issues = (1..=7)
        .map(|number| issue_for(repository, number, "open", &[], &[]))
        .collect();
    let dependencies = vec![
        (1, vec![internal_blocker_for(repository, 2, "open")]),
        (2, vec![internal_blocker_for(repository, 1, "open")]),
        (3, vec![internal_blocker_for(repository, 1, "open")]),
        (4, vec![external_blocker("private/roadmap", 80, "unknown")]),
        (5, vec![internal_blocker_for(repository, 4, "open")]),
        (6, vec![internal_blocker_for(repository, 99, "open")]),
        (7, vec![external_blocker("other/repo", 70, "closed")]),
    ];
    let (state, api_url) = seeded_replica(repository, issues, dependencies);

    let output = grit_command_for(&state, &api_url, repository, "plan", None)
        .output()
        .expect("plan with unresolved boundaries");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("plan JSON");

    assert_eq!(layer_numbers(&output), vec![vec![7]]);
    assert_eq!(unresolved_numbers(&output), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(unresolved_reasons(&output, 1), vec!["cycle"]);
    assert_eq!(unresolved_reasons(&output, 2), vec!["cycle"]);
    assert_eq!(
        unresolved_reasons(&output, 3),
        vec!["depends_on_unresolved"]
    );
    assert_eq!(
        unresolved_reasons(&output, 4),
        vec!["opaque_external_blocker"]
    );
    assert_eq!(
        unresolved_reasons(&output, 5),
        vec!["depends_on_unresolved"]
    );
    assert_eq!(
        unresolved_reasons(&output, 6),
        vec!["unknown_internal_blocker"]
    );
}

#[test]
fn human_plan_describes_topology_without_scheduling_claims() {
    let repository = "acme/human-plan";
    let issues = vec![
        issue_for(repository, 1, "open", &[], &[]),
        issue_for(repository, 2, "open", &[], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![internal_blocker_for(repository, 1, "open")]),
    ];
    let (state, api_url) = seeded_replica(repository, issues, dependencies);
    let human = human_plan_command(&state, &api_url, repository)
        .output()
        .expect("human plan");
    assert_success(&human);
    let human = String::from_utf8(human.stdout).expect("UTF-8 human output");

    assert!(human.contains("dependency layers (counterfactual topology):"));
    assert!(human.contains("layer 1: #2 (execution-scope eligible after blockers)"));
    for unsupported_concept in ["worker wave", "eta", "critical path", "calendar"] {
        assert!(
            !human.to_ascii_lowercase().contains(unsupported_concept),
            "human plan must not claim {unsupported_concept}: {human}"
        );
    }
}

fn issue_numbers(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .expect("Issue array")
        .iter()
        .map(|issue| issue["number"].as_u64().expect("Issue number"))
        .collect()
}

fn layer_numbers(output: &Value) -> Vec<Vec<u64>> {
    output["dependency_layers"]["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .map(|layer| issue_numbers(&layer["issues"]))
        .collect()
}

fn unresolved_numbers(output: &Value) -> Vec<u64> {
    output["dependency_layers"]["unresolved"]
        .as_array()
        .expect("unresolved Issues")
        .iter()
        .map(|entry| entry["issue"]["number"].as_u64().expect("Issue number"))
        .collect()
}

fn unresolved_reasons(output: &Value, number: u64) -> Vec<&str> {
    output["dependency_layers"]["unresolved"]
        .as_array()
        .expect("unresolved Issues")
        .iter()
        .find(|entry| entry["issue"]["number"] == number)
        .expect("unresolved Issue")["reasons"]
        .as_array()
        .expect("unresolved reasons")
        .iter()
        .map(|reason| reason.as_str().expect("reason"))
        .collect()
}

fn layer(output: &Value, index: u64) -> &Value {
    output["dependency_layers"]["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .find(|layer| layer["index"] == index)
        .expect("layer")
}

fn issue_annotation(layer: &Value, number: u64, field: &str) -> bool {
    layer["issues"]
        .as_array()
        .expect("layer Issues")
        .iter()
        .find(|issue| issue["number"] == number)
        .expect("Issue in layer")[field]
        .as_bool()
        .expect("boolean annotation")
}

fn seeded_replica(
    repository: &str,
    issues: Vec<Value>,
    dependencies: Vec<(u64, Vec<Value>)>,
) -> (TempDir, String) {
    let mut github = Server::new();
    let mocks = mock_repository(&mut github, repository, issues, dependencies);
    let state = TempDir::new().expect("temporary state directory");
    let api_url = github.url();
    let seed = grit_command_for(&state, &api_url, repository, "ready", None)
        .output()
        .expect("seed Local replica");
    assert_success(&seed);
    mocks.assert();
    drop(github);
    (state, api_url)
}

fn grit_command(state: &TempDir, api_url: &str, command: &str) -> Command {
    grit_command_for(state, api_url, "acme/plan", command, None)
}

fn grit_command_for(
    state: &TempDir,
    api_url: &str,
    repository: &str,
    command: &str,
    assignee: Option<&str>,
) -> Command {
    let mut grit = Command::new(env!("CARGO_BIN_EXE_grit"));
    grit.args([command, "--repo", repository, "--json"]);
    if let Some(assignee) = assignee {
        grit.args(["--assignee", assignee]);
    }
    grit.env("GRIT_NO_KEYRING", "1");
    grit.env("GRIT_STATE_DIR", state.path());
    grit.env("GRIT_GITHUB_API_URL", api_url);
    grit.env("GH_TOKEN", "test-token");
    grit
}

fn human_plan_command(state: &TempDir, api_url: &str, repository: &str) -> Command {
    let mut grit = Command::new(env!("CARGO_BIN_EXE_grit"));
    grit.args(["plan", "--repo", repository]);
    grit.env("GRIT_NO_KEYRING", "1");
    grit.env("GRIT_STATE_DIR", state.path());
    grit.env("GRIT_GITHUB_API_URL", api_url);
    grit.env("GH_TOKEN", "test-token");
    grit
}
