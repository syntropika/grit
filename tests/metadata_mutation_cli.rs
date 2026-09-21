use std::{
    collections::BTreeSet,
    process::Command,
    sync::{Arc, Mutex},
};

use mockito::{Matcher, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn canonical_priority_labels_are_rejected_by_the_generic_label_path() {
    let state = TempDir::new().expect("state directory");
    let unavailable = Server::new();
    let output = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["label", "acme/widgets#1", "--add", "Priority:P0", "--json"])
        .output()
        .expect("reject Priority label");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--priority"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn offline_generic_label_uses_set_semantics_and_projects_into_the_working_graph() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_replica(&mut github, &state);
    let replica_path = state.path().join("repositories/acme/widgets/replica.json");
    let synchronized = std::fs::read(&replica_path).expect("replica");
    let unavailable = Server::new();

    let first = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["label", "acme/widgets#1", "--add", "area:core", "--json"])
        .output()
        .expect("queue generic label");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("label JSON");
    assert_eq!(first["pending"], true);
    assert_eq!(first["label"], "area:core");
    assert_eq!(first["desired_present"], true);

    let ready = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["ready", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("project generic label");
    assert_success(&ready);
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("ready JSON");
    assert_eq!(ready["issues"][0]["labels"], json!(["area:core"]));
    assert_eq!(ready["issues"][0]["pending"], true);
    assert_eq!(
        std::fs::read(replica_path).expect("unchanged replica"),
        synchronized
    );
}

#[test]
fn pending_parent_relationship_never_changes_dependency_readiness_or_ranking() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_replica(&mut github, &state);
    let unavailable = Server::new();

    let before = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["next", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("rank before parent relation");
    assert_success(&before);
    let before: Value = serde_json::from_slice(&before.stdout).expect("next JSON");

    let parent = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args([
            "sub-issue",
            "acme/widgets#1",
            "--add",
            "acme/widgets#2",
            "--json",
        ])
        .output()
        .expect("queue parent relation");
    assert_success(&parent);
    let parent: Value = serde_json::from_slice(&parent.stdout).expect("parent JSON");
    assert_eq!(parent["pending"], true);
    assert_eq!(parent["relationship"]["kind"], "parent_of");

    let after = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["next", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("rank after parent relation");
    assert_success(&after);
    let after: Value = serde_json::from_slice(&after.stdout).expect("next JSON");
    assert_eq!(
        after["recommendation"]["first_issue"]["number"],
        before["recommendation"]["first_issue"]["number"]
    );
    let replica: Value = serde_json::from_slice(
        &std::fs::read(state.path().join("repositories/acme/widgets/replica.json"))
            .expect("replica"),
    )
    .expect("replica JSON");
    assert_eq!(replica["dependencies"], json!([]));
}

#[test]
fn repeated_online_generic_label_add_is_idempotent_and_never_uses_priority_replacement() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state);
    let remote = Arc::new(Mutex::new(RemoteRepository {
        issues: vec![issue(1), issue(2)],
        ..RemoteRepository::default()
    }));
    let mut github = Server::new();
    let fetches = mock_issue_fetch(&mut github, Arc::clone(&remote), 2);
    let writes = mock_generic_label_add(&mut github, Arc::clone(&remote), 1);
    let inventories = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 2, 4);

    let first = grit(&state, &github.url())
        .args(["label", "acme/widgets#1", "--add", "area:core", "--json"])
        .output()
        .expect("add generic label");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("first label JSON");
    assert_eq!(first["result"], "added");

    let second = grit(&state, &github.url())
        .args(["label", "acme/widgets#1", "--add", "AREA:CORE", "--json"])
        .output()
        .expect("repeat generic label");
    assert_success(&second);
    let second: Value = serde_json::from_slice(&second.stdout).expect("second label JSON");
    assert_eq!(second["result"], "already_present");
    assert_eq!(remote.lock().expect("remote").label_writes, 1);
    fetches.assert();
    writes.assert();
    inventories.assert();
}

#[test]
fn duplicate_parent_intents_for_two_drafts_replay_once_after_identity_mapping() {
    let state = TempDir::new().expect("state directory");
    let remote = Arc::new(Mutex::new(RemoteRepository::default()));
    let mut seed = Server::new();
    let inventory = mock_dynamic_inventory(&mut seed, Arc::clone(&remote), 1, 0);
    let synced = grit(&state, &seed.url())
        .args(["sync", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("seed empty replica");
    assert_success(&synced);
    inventory.assert();

    let parent = queue_draft(&state, &seed.url(), "Parent");
    let child = queue_draft(&state, &seed.url(), "Child");
    let unavailable = Server::new();
    for _ in 0..2 {
        let queued = grit(&state, &unavailable.url())
            .env_remove("GH_TOKEN")
            .args([
                "sub-issue",
                parent.as_str(),
                "--add",
                child.as_str(),
                "--json",
            ])
            .output()
            .expect("queue duplicate parent relation");
        assert_success(&queued);
    }
    let queued_label = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["label", child.as_str(), "--add", "area:draft", "--json"])
        .output()
        .expect("queue Draft label");
    assert_success(&queued_label);

    let mut github = Server::new();
    let inventory = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 2, 2);
    let creates = mock_issue_creates(&mut github, Arc::clone(&remote), 2);
    let child_fetch = mock_issue_fetch_number(&mut github, Arc::clone(&remote), 2, 1);
    let subissue_reads = mock_sub_issue_reads(&mut github, Arc::clone(&remote), 1, 2);
    let parent_write = mock_parent_add(&mut github, Arc::clone(&remote), 1);
    let label_write =
        mock_label_add_for_issue(&mut github, Arc::clone(&remote), 2, "area:draft", 1);
    let reconciled = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("reconcile Draft parent relations");
    assert_success(&reconciled);
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout).expect("reconcile JSON");
    assert_eq!(reconciled["summary"]["applied"], 4, "{reconciled}");
    assert_eq!(reconciled["summary"]["already_satisfied"], 1);
    assert_eq!(reconciled["summary"]["remaining"], 0);
    let remote = remote.lock().expect("remote");
    assert_eq!(remote.parent_writes, 1);
    assert!(remote.parent_relations.contains(&(1, 200)));
    assert_eq!(remote.issues[1]["labels"][0]["name"], "area:draft");
    drop(remote);
    let replica: Value = serde_json::from_slice(
        &std::fs::read(state.path().join("repositories/acme/widgets/replica.json"))
            .expect("replica"),
    )
    .expect("replica JSON");
    assert_eq!(replica["dependencies"], json!([]));
    inventory.assert();
    creates.assert();
    child_fetch.assert();
    subissue_reads.assert();
    parent_write.assert();
    label_write.assert();
}

#[test]
fn ordered_label_add_then_remove_reconciles_in_one_pass() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state);
    let unavailable = Server::new();
    for arguments in [
        ["label", "acme/widgets#1", "--add", "area:core", "--json"],
        ["label", "acme/widgets#1", "--remove", "AREA:CORE", "--json"],
    ] {
        let queued = grit(&state, &unavailable.url())
            .env_remove("GH_TOKEN")
            .args(arguments)
            .output()
            .expect("queue ordered label intent");
        assert_success(&queued);
    }

    let remote = Arc::new(Mutex::new(RemoteRepository {
        issues: vec![issue(1), issue(2)],
        ..RemoteRepository::default()
    }));
    let mut github = Server::new();
    let inventories = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 2, 4);
    let fetches = mock_issue_fetch(&mut github, Arc::clone(&remote), 2);
    let add = mock_generic_label_add(&mut github, Arc::clone(&remote), 1);
    let remove = mock_generic_label_remove(&mut github, Arc::clone(&remote), 1);
    let reconciled = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("reconcile ordered label intents");
    assert_success(&reconciled);
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout).expect("reconcile JSON");
    assert_eq!(reconciled["summary"]["applied"], 2, "{reconciled}");
    assert_eq!(reconciled["summary"]["remaining"], 0);
    assert_eq!(
        remote.lock().expect("remote").issues[0]["labels"],
        json!([])
    );
    inventories.assert();
    fetches.assert();
    add.assert();
    remove.assert();
}

#[test]
fn online_parent_removal_is_idempotent_set_mutation_not_a_dependency() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state);
    let remote = Arc::new(Mutex::new(RemoteRepository {
        issues: vec![issue(1), issue(2)],
        parent_relations: BTreeSet::from([(1, 200)]),
        ..RemoteRepository::default()
    }));
    let mut github = Server::new();
    let child = mock_issue_fetch_number(&mut github, Arc::clone(&remote), 2, 1);
    let subissues = mock_sub_issue_reads(&mut github, Arc::clone(&remote), 1, 2);
    let remove = mock_parent_remove(&mut github, Arc::clone(&remote), 1);
    let inventory = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 1, 2);
    let output = grit(&state, &github.url())
        .args([
            "sub-issue",
            "acme/widgets#1",
            "--remove",
            "acme/widgets#2",
            "--json",
        ])
        .output()
        .expect("remove parent relationship");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("parent JSON");
    assert_eq!(output["result"], "removed");
    assert_eq!(output["pending"], false);
    let remote = remote.lock().expect("remote");
    assert!(remote.parent_relations.is_empty());
    assert_eq!(remote.parent_writes, 1);
    drop(remote);
    let replica: Value = serde_json::from_slice(
        &std::fs::read(state.path().join("repositories/acme/widgets/replica.json"))
            .expect("replica"),
    )
    .expect("replica JSON");
    assert_eq!(replica["dependencies"], json!([]));
    child.assert();
    subissues.assert();
    remove.assert();
    inventory.assert();
}

#[test]
fn ambiguous_label_write_is_queued_but_rejected_write_is_not() {
    for (status, should_queue) in [(500, true), (422, false)] {
        let state = TempDir::new().expect("state directory");
        let mut seed = Server::new();
        seed_replica(&mut seed, &state);
        let remote = Arc::new(Mutex::new(RemoteRepository {
            issues: vec![issue(1), issue(2)],
            ..RemoteRepository::default()
        }));
        let mut github = Server::new();
        let fetches = mock_issue_fetch(&mut github, Arc::clone(&remote), 2);
        let write = github
            .mock("POST", "/repos/acme/widgets/issues/1/labels")
            .match_body(Matcher::Json(json!({"labels": ["area:core"]})))
            .with_status(status)
            .expect(1)
            .create();
        let output = grit(&state, &github.url())
            .args(["label", "acme/widgets#1", "--add", "area:core", "--json"])
            .output()
            .expect("attempt label mutation");
        assert_eq!(output.status.success(), should_queue, "HTTP {status}");
        if should_queue {
            let output: Value = serde_json::from_slice(&output.stdout).expect("pending JSON");
            assert_eq!(output["pending"], true);
            let outbox = load_outbox(&state);
            assert_eq!(outbox["operations"].as_array().unwrap().len(), 1);
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains("422"));
            assert!(
                !state
                    .path()
                    .join("repositories/acme/widgets/outbox.json")
                    .exists()
            );
        }
        fetches.assert();
        write.assert();
    }
}

#[test]
fn successful_remote_label_write_with_failed_readback_keeps_local_state_unchanged() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state);
    let replica_path = state.path().join("repositories/acme/widgets/replica.json");
    let before = std::fs::read(&replica_path).expect("replica before mutation");
    let remote = Arc::new(Mutex::new(RemoteRepository {
        issues: vec![issue(1), issue(2)],
        ..RemoteRepository::default()
    }));
    let mut github = Server::new();
    let fetch = mock_issue_fetch(&mut github, Arc::clone(&remote), 1);
    let write = mock_generic_label_add(&mut github, Arc::clone(&remote), 1);
    let failed_readback = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(500)
        .expect(1)
        .create();
    let output = grit(&state, &github.url())
        .args(["label", "acme/widgets#1", "--add", "area:core", "--json"])
        .output()
        .expect("mutate with failed readback");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("synchronization failed"));
    assert_eq!(
        std::fs::read(&replica_path).expect("replica after failure"),
        before
    );
    assert_eq!(remote.lock().expect("remote").label_writes, 1);
    assert!(
        !state
            .path()
            .join("repositories/acme/widgets/outbox.json")
            .exists()
    );
    fetch.assert();
    write.assert();
    failed_readback.assert();
}

#[test]
fn corrupt_persisted_metadata_operand_fails_closed_without_a_panic() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_replica(&mut github, &state);
    let unavailable = Server::new();
    let queued = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["label", "acme/widgets#1", "--add", "area:core", "--json"])
        .output()
        .expect("queue label");
    assert_success(&queued);
    let path = state.path().join("repositories/acme/widgets/outbox.json");
    let mut outbox = load_outbox(&state);
    outbox["operations"][0]["target"]["issue"]["number"] = json!(u64::MAX);
    std::fs::write(&path, serde_json::to_vec_pretty(&outbox).unwrap()).expect("corrupt outbox");

    let output = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["ready", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("read corrupt outbox");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid Issue identity"), "{stderr}");
    assert!(
        !stderr.to_ascii_lowercase().contains("panicked"),
        "{stderr}"
    );
}

#[test]
fn concurrent_generic_label_queues_both_survive_restart() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_replica(&mut github, &state);
    let unavailable = Server::new();
    let mut first = grit(&state, &unavailable.url());
    first
        .env_remove("GH_TOKEN")
        .args(["label", "acme/widgets#1", "--add", "area:first", "--json"]);
    let mut second = grit(&state, &unavailable.url());
    second.env_remove("GH_TOKEN").args([
        "label",
        "acme/widgets#1",
        "--add",
        "area:second",
        "--json",
    ]);
    let first = std::thread::spawn(move || first.output().expect("first concurrent label"));
    let second = std::thread::spawn(move || second.output().expect("second concurrent label"));
    assert_success(&first.join().expect("first thread"));
    assert_success(&second.join().expect("second thread"));

    let outbox = load_outbox(&state);
    assert_eq!(outbox["operations"].as_array().unwrap().len(), 2);
    let ready = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["ready", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("reload concurrent labels");
    assert_success(&ready);
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("ready JSON");
    assert_eq!(
        ready["issues"][0]["labels"],
        json!(["area:first", "area:second"])
    );
}

fn load_outbox(state: &TempDir) -> Value {
    serde_json::from_slice(
        &std::fs::read(state.path().join("repositories/acme/widgets/outbox.json")).expect("outbox"),
    )
    .expect("outbox JSON")
}

fn seed_replica(github: &mut Server, state: &TempDir) {
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let issues = Arc::new(Mutex::new(vec![issue(1), issue(2)]));
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let issue_state = Arc::clone(&issues);
    let issue_inventory = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            json!(*issue_state.lock().expect("issue state"))
                .to_string()
                .into_bytes()
        })
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependencies = github
        .mock(
            "GET",
            Matcher::Regex(r"^/repos/acme/widgets/issues/[0-9]+/dependencies/blocked_by$".into()),
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(2)
        .create();
    let output = grit(state, &github.url())
        .args(["sync", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("seed replica");
    assert_success(&output);
    labels.assert();
    events.assert();
    issue_inventory.assert();
    comments.assert();
    dependencies.assert();
}

#[derive(Default)]
struct RemoteRepository {
    issues: Vec<Value>,
    parent_relations: BTreeSet<(u64, u64)>,
    label_writes: usize,
    parent_writes: usize,
}

struct RepositoryMocks {
    mocks: Vec<mockito::Mock>,
}

impl RepositoryMocks {
    fn assert(self) {
        for mock in self.mocks {
            mock.assert();
        }
    }
}

fn mock_dynamic_inventory(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    inventories: usize,
    dependency_reads: usize,
) -> RepositoryMocks {
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(inventories)
        .create();
    let events = github
        .mock("GET", "/repos/acme/widgets/issues/events")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(inventories)
        .create();
    let issue_state = Arc::clone(&remote);
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
        .with_body_from_request(move |_| {
            json!(issue_state.lock().expect("remote").issues)
                .to_string()
                .into_bytes()
        })
        .expect(inventories)
        .create();
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .expect(inventories)
        .create();
    let mut mocks = vec![labels, events, issues, comments];
    if dependency_reads > 0 {
        mocks.push(
            github
                .mock(
                    "GET",
                    Matcher::Regex(
                        r"^/repos/acme/widgets/issues/[0-9]+/dependencies/blocked_by$".into(),
                    ),
                )
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body("[]")
                .expect(dependency_reads)
                .create(),
        );
    }
    RepositoryMocks { mocks }
}

fn mock_issue_fetch(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    mock_issue_fetch_number(github, remote, 1, expected)
}

fn mock_issue_fetch_number(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    number: u64,
    expected: usize,
) -> RepositoryMocks {
    let issue_state = Arc::clone(&remote);
    let mock = github
        .mock(
            "GET",
            format!("/repos/acme/widgets/issues/{number}").as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            issue_state.lock().expect("remote").issues[(number - 1) as usize]
                .to_string()
                .into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn mock_generic_label_add(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    let label_state = Arc::clone(&remote);
    let mock = github
        .mock("POST", "/repos/acme/widgets/issues/1/labels")
        .match_body(Matcher::Json(json!({"labels": ["area:core"]})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let mut remote = label_state.lock().expect("remote");
            remote.label_writes += 1;
            remote.issues[0]["labels"] = json!([{
                "id": 400,
                "node_id": "L_area",
                "name": "area:core",
                "color": "123456",
                "description": "Core"
            }]);
            remote.issues[0]["labels"].to_string().into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn mock_generic_label_remove(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    let label_state = Arc::clone(&remote);
    let mock = github
        .mock("DELETE", "/repos/acme/widgets/issues/1/labels/area:core")
        .with_status(200)
        .with_body_from_request(move |_| {
            let mut remote = label_state.lock().expect("remote");
            remote.label_writes += 1;
            remote.issues[0]["labels"] = json!([]);
            Vec::new()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn mock_label_add_for_issue(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    number: u64,
    label: &'static str,
    expected: usize,
) -> RepositoryMocks {
    let label_state = Arc::clone(&remote);
    let mock = github
        .mock(
            "POST",
            format!("/repos/acme/widgets/issues/{number}/labels").as_str(),
        )
        .match_body(Matcher::Json(json!({"labels": [label]})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let mut remote = label_state.lock().expect("remote");
            remote.label_writes += 1;
            remote.issues[(number - 1) as usize]["labels"] = json!([{
                "id": 401,
                "node_id": "L_draft",
                "name": label,
                "color": "123456",
                "description": "Draft"
            }]);
            remote.issues[(number - 1) as usize]["labels"]
                .to_string()
                .into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn queue_draft(state: &TempDir, api_url: &str, title: &str) -> String {
    let output = grit(state, api_url)
        .args([
            "create",
            "--repo",
            "acme/widgets",
            "--title",
            title,
            "--json",
        ])
        .output()
        .expect("queue Draft Issue");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("Draft JSON");
    output["draft"]["key"]
        .as_str()
        .expect("Draft key")
        .to_owned()
}

fn mock_issue_creates(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    let create_state = Arc::clone(&remote);
    let mock = github
        .mock("POST", "/repos/acme/widgets/issues")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |request| {
            let body: Value =
                serde_json::from_slice(request.body().expect("create body")).expect("create JSON");
            let mut remote = create_state.lock().expect("remote");
            let number = remote.issues.len() as u64 + 1;
            let mut created = issue(number);
            created["title"] = body["title"].clone();
            created["body"] = body["body"].clone();
            remote.issues.push(created.clone());
            created.to_string().into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn mock_sub_issue_reads(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    parent_number: u64,
    expected: usize,
) -> RepositoryMocks {
    let relation_state = Arc::clone(&remote);
    let mock = github
        .mock(
            "GET",
            format!("/repos/acme/widgets/issues/{parent_number}/sub_issues").as_str(),
        )
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let remote = relation_state.lock().expect("remote");
            let children: Vec<_> = remote
                .parent_relations
                .iter()
                .filter(|(parent, _)| *parent == parent_number)
                .filter_map(|(_, child_id)| {
                    remote
                        .issues
                        .iter()
                        .find(|issue| issue["id"].as_u64() == Some(*child_id))
                        .cloned()
                })
                .collect();
            json!(children).to_string().into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn mock_parent_add(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    let relation_state = Arc::clone(&remote);
    let mock = github
        .mock("POST", "/repos/acme/widgets/issues/1/sub_issues")
        .match_body(Matcher::Json(json!({"sub_issue_id": 200})))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let mut remote = relation_state.lock().expect("remote");
            remote.parent_writes += 1;
            remote.parent_relations.insert((1, 200));
            remote.issues[1].to_string().into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn mock_parent_remove(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    let relation_state = Arc::clone(&remote);
    let mock = github
        .mock("DELETE", "/repos/acme/widgets/issues/1/sub_issue")
        .match_body(Matcher::Json(json!({"sub_issue_id": 200})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let mut remote = relation_state.lock().expect("remote");
            remote.parent_writes += 1;
            remote.parent_relations.remove(&(1, 200));
            remote.issues[1].to_string().into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks { mocks: vec![mock] }
}

fn issue(number: u64) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": "open",
        "state_reason": null,
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": null,
        "assignees": [],
        "labels": [],
        "created_at": "2026-08-07T00:00:00Z",
        "updated_at": "2026-08-07T00:00:00Z",
        "closed_at": null,
        "pull_request": null
    })
}

fn grit(state: &TempDir, api_url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command
        .env("GRIT_STATE_DIR", state.path())
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GH_TOKEN", "test-token");
    command
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
