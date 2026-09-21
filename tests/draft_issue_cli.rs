use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use mockito::{Matcher, Request, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn offline_create_returns_a_stable_draft_identity_and_participates_in_next() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_empty_replica(&mut github, &state);

    let created = grit(&state, &github.url())
        .args([
            "create",
            "--repo",
            "acme/widgets",
            "--title",
            "Draft the migration",
            "--body",
            "Keep this body local until reconciliation.",
            "--json",
        ])
        .output()
        .expect("queue Draft Issue");
    assert_success(&created);
    let created: Value = serde_json::from_slice(&created.stdout).expect("create JSON");
    assert_eq!(created["schema_version"], "grit.issue-create/v1");
    assert_eq!(created["pending"], true);
    let temporary_id = created["draft"]["temporary_id"]
        .as_str()
        .expect("Temporary Issue ID");
    assert!(!temporary_id.is_empty());
    assert_eq!(
        created["draft"]["stable_node_key"],
        json!([1, temporary_id])
    );
    let stable_key = created["draft"]["key"]
        .as_str()
        .expect("Draft reference key");
    assert_eq!(stable_key, format!("acme/widgets#draft:{temporary_id}"));
    let outbox: Value = serde_json::from_slice(
        &fs::read(state.path().join("repositories/acme/widgets/outbox.json")).expect("outbox"),
    )
    .expect("outbox JSON");
    let marker = outbox["operations"][0]["marker"]
        .as_str()
        .expect("persisted marker");
    assert!(!created.to_string().contains(marker));

    let unavailable = Server::new();
    let ready = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["ready", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("enumerate Draft Issue offline");
    assert_success(&ready);
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("ready JSON");
    assert_eq!(ready["issues"][0]["key"], stable_key);
    assert_eq!(ready["issues"][0]["temporary_id"], temporary_id);

    let ranked = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["next", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("rank Draft Issue offline");
    assert_success(&ranked);
    let ranked: Value = serde_json::from_slice(&ranked.stdout).expect("next JSON");
    assert_eq!(ranked["source"], "local_fallback");
    assert_eq!(ranked["pending"], true);
    assert_eq!(ranked["recommendation"]["first_issue"]["key"], stable_key);
    assert_eq!(
        ranked["recommendation"]["first_issue"]["temporary_id"],
        temporary_id
    );
    assert_eq!(ranked["recommendation"]["first_issue"]["pending"], true);
    assert_eq!(
        ranked["recommendation"]["first_issue"]["title"],
        "Draft the migration"
    );
    assert!(!ranked.to_string().contains(marker));
}

#[test]
fn multistep_draft_rollouts_follow_stable_ids_instead_of_synthetic_numbers() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_empty_replica(&mut github, &state);
    for title in ["First", "Second", "Third"] {
        queue_draft(&state, &github.url(), title);
    }
    let path = state.path().join("repositories/acme/widgets/outbox.json");
    let mut outbox: Value =
        serde_json::from_slice(&fs::read(&path).expect("outbox")).expect("JSON");
    let identities = [
        (
            "70000000-0000-4000-8000-000000000001",
            0xf000000000004000_u64,
        ),
        (
            "80000000-0000-4000-8000-000000000002",
            0x8000000000004000_u64,
        ),
        (
            "90000000-0000-4000-8000-000000000003",
            0x9000000000004000_u64,
        ),
    ];
    for (operation, (temporary_id, synthetic_number)) in outbox["operations"]
        .as_array_mut()
        .expect("operations")
        .iter_mut()
        .zip(identities)
    {
        operation["temporary_id"] = json!(temporary_id);
        operation["synthetic_number"] = json!(synthetic_number);
    }
    fs::write(path, serde_json::to_vec(&outbox).expect("outbox JSON")).expect("save identities");
    let output = grit(&state, &github.url())
        .env_remove("GH_TOKEN")
        .args(["next", "--repo", "acme/widgets", "--horizon", "3", "--json"])
        .output()
        .expect("rank Draft sequence");
    assert_success(&output);
    let ranked: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    let actual: Vec<_> = ranked["recommendation"]["rollout"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| {
            step["issue"]["temporary_id"]
                .as_str()
                .expect("Draft identity")
        })
        .collect();
    assert_eq!(actual, identities.map(|(id, _)| id));
    assert_eq!(
        ranked["comparison_to_runner_up"]["component"],
        "stable_node_key"
    );
}

#[test]
fn two_related_drafts_map_to_two_github_issues_and_one_native_dependency() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_empty_replica(&mut seed, &state);
    let first = queue_draft(&state, &seed.url(), "First Draft");
    let second = queue_draft(&state, &seed.url(), "Second Draft");
    let first_key = first["draft"]["key"].as_str().expect("first key");
    let second_key = second["draft"]["key"].as_str().expect("second key");
    let first_temporary_id = first["draft"]["temporary_id"].clone();
    let second_temporary_id = second["draft"]["temporary_id"].clone();

    let queued_edge = grit(&state, &seed.url())
        .args(["block", first_key, "--by", second_key, "--json"])
        .output()
        .expect("queue Draft Dependency");
    assert_success(&queued_edge);
    let queued_edge: Value = serde_json::from_slice(&queued_edge.stdout).expect("Dependency JSON");
    assert_eq!(queued_edge["pending"], true);
    assert_eq!(
        queued_edge["operation"]["depends_on"]
            .as_array()
            .expect("creation dependencies")
            .len(),
        2
    );
    let unavailable = Server::new();
    let ranked = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["next", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("rank related Drafts offline");
    assert_success(&ranked);
    let ranked: Value = serde_json::from_slice(&ranked.stdout).expect("next JSON");
    assert_eq!(ranked["recommendation"]["first_issue"]["key"], second_key);
    assert_eq!(
        ranked["recommendation"]["outcome"]["unlocks"][0]["issue"]["key"],
        first_key
    );

    let outbox_path = state.path().join("repositories/acme/widgets/outbox.json");
    let outbox: Value =
        serde_json::from_slice(&fs::read(&outbox_path).expect("outbox")).expect("outbox JSON");
    let markers: Vec<_> = outbox["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .take(2)
        .map(|operation| operation["marker"].as_str().expect("marker").to_owned())
        .collect();

    let mut github = Server::new();
    let remote = Arc::new(Mutex::new(DraftRemote::default()));
    let mocks = mock_draft_repository(&mut github, Arc::clone(&remote));
    let reconciled = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("reconcile Drafts");
    assert_success(&reconciled);
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout).expect("reconcile JSON");
    assert_eq!(reconciled["summary"]["applied"], 3, "{reconciled}");
    assert_eq!(reconciled["summary"]["remaining"], 0);
    assert_eq!(reconciled["operations"][2]["edge"]["blocked_number"], 1);
    assert_eq!(reconciled["operations"][2]["edge"]["blocker_number"], 2);
    assert_eq!(
        reconciled["operations"][2]["edge"]["blocked_temporary_id"],
        first_temporary_id
    );
    assert_eq!(
        reconciled["operations"][2]["edge"]["blocker_temporary_id"],
        second_temporary_id
    );

    let remote = remote.lock().expect("remote lock");
    assert_eq!(remote.issues.len(), 2);
    assert!(remote.dependency_present);
    assert_eq!(remote.create_requests.len(), 2);
    assert!(
        remote.create_requests[0]["body"]
            .as_str()
            .is_some_and(|body| body.contains(&markers[0]))
    );
    assert!(
        remote.create_requests[1]["body"]
            .as_str()
            .is_some_and(|body| body.contains(&markers[1]))
    );
    drop(remote);

    let identities: Value = serde_json::from_slice(
        &fs::read(
            state
                .path()
                .join("repositories/acme/widgets/draft-identities.json"),
        )
        .expect("identity map"),
    )
    .expect("identity JSON");
    assert_eq!(
        identities["identities"]
            .as_object()
            .expect("identity entries")
            .len(),
        2
    );
    let persisted_outbox: Value =
        serde_json::from_slice(&fs::read(outbox_path).expect("retired outbox"))
            .expect("retired outbox JSON");
    assert_eq!(persisted_outbox["operations"], json!([]));
    let replica: Value = serde_json::from_slice(
        &fs::read(state.path().join("repositories/acme/widgets/replica.json")).expect("replica"),
    )
    .expect("replica JSON");
    assert_eq!(replica["issues"].as_array().expect("Issues").len(), 2);
    for issue in replica["issues"].as_array().expect("Issues") {
        assert!(
            !issue["body"]
                .as_str()
                .expect("visible body")
                .contains("grit-operation")
        );
    }
    mocks.assert();
}

#[test]
fn accepted_create_with_a_failed_response_is_found_by_marker_without_replay() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_empty_replica(&mut seed, &state);
    let created = queue_draft(&state, &seed.url(), "Ambiguous Draft");
    let temporary_id = created["draft"]["temporary_id"]
        .as_str()
        .expect("temporary ID")
        .to_owned();

    let mut github = Server::new();
    let remote = Arc::new(Mutex::new(DraftRemote::default()));
    let mocks = mock_ambiguous_create_repository(
        &mut github,
        Arc::clone(&remote),
        AmbiguousCreateScenario::new(1, 1),
    );
    let first = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("ambiguous create pass");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("first reconcile JSON");
    assert_eq!(first["summary"]["failed"], 1);
    assert_eq!(first["summary"]["remaining"], 1);
    assert_eq!(remote.lock().expect("remote lock").issues.len(), 1);

    let recovered = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("marker recovery pass");
    assert_success(&recovered);
    let recovered: Value =
        serde_json::from_slice(&recovered.stdout).expect("recovered reconcile JSON");
    assert_eq!(recovered["summary"]["already_satisfied"], 1);
    assert_eq!(recovered["summary"]["remaining"], 0);
    let remote = remote.lock().expect("remote lock");
    assert_eq!(remote.issues.len(), 1);
    assert_eq!(remote.create_requests.len(), 1);
    drop(remote);

    let identities: Value = serde_json::from_slice(
        &fs::read(
            state
                .path()
                .join("repositories/acme/widgets/draft-identities.json"),
        )
        .expect("identity map"),
    )
    .expect("identity JSON");
    assert_eq!(identities["identities"][&temporary_id]["issue_number"], 1);
    mocks.assert();
}

#[test]
fn zero_or_multiple_marker_matches_leave_the_create_unresolved_without_replay() {
    for accepted_copies in [0_usize, 2] {
        let state = TempDir::new().expect("state directory");
        let mut seed = Server::new();
        seed_empty_replica(&mut seed, &state);
        queue_draft(&state, &seed.url(), "Unresolved Draft");

        let mut github = Server::new();
        let remote = Arc::new(Mutex::new(DraftRemote::default()));
        let mocks = mock_ambiguous_create_repository(
            &mut github,
            Arc::clone(&remote),
            AmbiguousCreateScenario::new(accepted_copies, 1),
        );
        let first = grit(&state, &github.url())
            .args(["reconcile", "--repo", "acme/widgets", "--json"])
            .output()
            .expect("ambiguous create pass");
        assert_success(&first);
        let second = grit(&state, &github.url())
            .args(["reconcile", "--repo", "acme/widgets", "--json"])
            .output()
            .expect("marker recovery pass");
        assert_success(&second);
        let second: Value = serde_json::from_slice(&second.stdout).expect("reconcile JSON");
        assert_eq!(second["summary"]["remaining"], 1);
        assert_eq!(second["summary"]["failed"], 1);
        if accepted_copies == 2 {
            assert_eq!(second["summary"]["conflicting"], 1);
        }
        let remote = remote.lock().expect("remote lock");
        assert_eq!(remote.issues.len(), accepted_copies);
        assert_eq!(remote.create_requests.len(), 1);
        drop(remote);
        mocks.assert();
    }
}

#[test]
fn marker_recovery_indexes_the_repository_once_for_multiple_uncertain_drafts() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_empty_replica(&mut seed, &state);
    queue_draft(&state, &seed.url(), "First uncertain Draft");
    queue_draft(&state, &seed.url(), "Second uncertain Draft");

    let mut github = Server::new();
    let remote = Arc::new(Mutex::new(DraftRemote::default()));
    let mocks = mock_ambiguous_create_repository(
        &mut github,
        Arc::clone(&remote),
        AmbiguousCreateScenario::new(1, 2),
    );
    let first = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("ambiguous create pass");
    assert_success(&first);
    let first: Value = serde_json::from_slice(&first.stdout).expect("first reconcile JSON");
    assert_eq!(first["summary"]["failed"], 2);

    let recovered = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("batched marker recovery pass");
    assert_success(&recovered);
    let recovered: Value =
        serde_json::from_slice(&recovered.stdout).expect("recovered reconcile JSON");
    assert_eq!(recovered["summary"]["already_satisfied"], 2);
    assert_eq!(recovered["summary"]["remaining"], 0);
    let remote = remote.lock().expect("remote lock");
    assert_eq!(remote.issues.len(), 2);
    assert_eq!(remote.create_requests.len(), 2);
    drop(remote);
    mocks.assert();
}

#[test]
fn one_remote_issue_cannot_be_mapped_to_two_draft_identities() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_empty_replica(&mut seed, &state);
    queue_draft(&state, &seed.url(), "First colliding Draft");
    queue_draft(&state, &seed.url(), "Second colliding Draft");
    let outbox: Value = serde_json::from_slice(
        &fs::read(state.path().join("repositories/acme/widgets/outbox.json")).expect("outbox"),
    )
    .expect("outbox JSON");
    let markers: Vec<_> = outbox["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operation| operation["marker"].as_str().expect("marker"))
        .collect();

    let mut github = Server::new();
    let remote = Arc::new(Mutex::new(DraftRemote::default()));
    let mocks = mock_ambiguous_create_repository(
        &mut github,
        Arc::clone(&remote),
        AmbiguousCreateScenario {
            accepted_copies: 0,
            expected_creates: 2,
            dependency_inventories: 2,
        },
    );
    let first = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("ambiguous create pass");
    assert_success(&first);

    let shared_body = json!(format!(
        "<!-- grit-operation:{} -->\n<!-- grit-operation:{} -->",
        markers[0], markers[1]
    ));
    remote
        .lock()
        .expect("remote lock")
        .issues
        .push(remote_issue(1, &json!("Shared Issue"), &shared_body));
    let recovered = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("colliding marker recovery pass");
    assert_success(&recovered);
    let recovered: Value = serde_json::from_slice(&recovered.stdout).expect("reconcile JSON");
    assert_eq!(recovered["summary"]["already_satisfied"], 1);
    assert_eq!(recovered["summary"]["conflicting"], 1);
    assert_eq!(recovered["summary"]["failed"], 1);
    assert_eq!(recovered["summary"]["remaining"], 1);
    assert!(
        recovered["operations"][1]["error"]
            .as_str()
            .is_some_and(|error| error.contains("already mapped"))
    );

    let identities: Value = serde_json::from_slice(
        &fs::read(
            state
                .path()
                .join("repositories/acme/widgets/draft-identities.json"),
        )
        .expect("identity map"),
    )
    .expect("identity JSON");
    assert_eq!(
        identities["identities"]
            .as_object()
            .expect("identity entries")
            .len(),
        1
    );
    assert_eq!(remote.lock().expect("remote lock").create_requests.len(), 2);
    mocks.assert();
}

fn queue_draft(state: &TempDir, api_url: &str, title: &str) -> Value {
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
        .expect("queue Draft");
    assert_success(&output);
    serde_json::from_slice(&output.stdout).expect("Draft JSON")
}

#[derive(Default)]
struct DraftRemote {
    issues: Vec<Value>,
    create_requests: Vec<Value>,
    dependency_present: bool,
}

struct DraftMocks {
    mocks: Vec<mockito::Mock>,
}

fn mock_ambiguous_create_repository(
    github: &mut Server,
    remote: Arc<Mutex<DraftRemote>>,
    scenario: AmbiguousCreateScenario,
) -> DraftMocks {
    let mut mocks = mock_inventory(
        github,
        Arc::clone(&remote),
        InventoryExpectations {
            labels: 4,
            issues: 5,
            comments: 4,
            dependency_inventories: scenario.dependency_inventories,
        },
    );
    let create_remote = Arc::clone(&remote);
    mocks.mocks.push(
        github
            .mock("POST", "/repos/acme/widgets/issues")
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let create: Value = serde_json::from_slice(request.body().expect("create body"))
                    .expect("create JSON");
                let mut remote = create_remote.lock().expect("remote lock");
                remote.create_requests.push(create.clone());
                let first_number = remote.issues.len() as u64 + 1;
                for number in first_number..first_number + scenario.accepted_copies as u64 {
                    remote
                        .issues
                        .push(remote_issue(number, &create["title"], &create["body"]));
                }
                json!({"message": "response lost after acceptance"})
                    .to_string()
                    .into_bytes()
            })
            .expect(scenario.expected_creates)
            .create(),
    );
    mocks
}

#[derive(Clone, Copy)]
struct AmbiguousCreateScenario {
    accepted_copies: usize,
    expected_creates: usize,
    dependency_inventories: usize,
}

impl AmbiguousCreateScenario {
    fn new(accepted_copies: usize, expected_creates: usize) -> Self {
        Self {
            accepted_copies,
            expected_creates,
            dependency_inventories: 3 * accepted_copies * expected_creates,
        }
    }
}

impl DraftMocks {
    fn assert(self) {
        for mock in self.mocks {
            mock.assert();
        }
    }
}

fn mock_draft_repository(github: &mut Server, remote: Arc<Mutex<DraftRemote>>) -> DraftMocks {
    let mut mocks = mock_inventory(
        github,
        Arc::clone(&remote),
        InventoryExpectations {
            labels: 2,
            issues: 2,
            comments: 2,
            dependency_inventories: 2,
        },
    );
    let creates = Arc::clone(&remote);
    mocks.mocks.push(
        github
            .mock("POST", "/repos/acme/widgets/issues")
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let create: Value = serde_json::from_slice(request.body().expect("create body"))
                    .expect("create JSON");
                let mut remote = creates.lock().expect("remote lock");
                let number = remote.issues.len() as u64 + 1;
                let issue = remote_issue(number, &create["title"], &create["body"]);
                remote.create_requests.push(create);
                remote.issues.push(issue.clone());
                issue.to_string().into_bytes()
            })
            .expect(2)
            .create(),
    );
    let individual = Arc::clone(&remote);
    mocks.mocks.push(
        github
            .mock(
                "GET",
                Matcher::Regex(r"^/repos/acme/widgets/issues/[0-9]+$".into()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let number = path_number(request);
                individual.lock().expect("remote lock").issues[(number - 1) as usize]
                    .to_string()
                    .into_bytes()
            })
            .expect(1)
            .create(),
    );
    let dependency_read = Arc::clone(&remote);
    mocks.mocks.push(
        github
            .mock(
                "GET",
                Matcher::Regex(
                    r"^/repos/acme/widgets/issues/[0-9]+/dependencies/blocked_by$".into(),
                ),
            )
            .match_query(Matcher::UrlEncoded("per_page".into(), "50".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let remote = dependency_read.lock().expect("remote lock");
                if remote.dependency_present && path_number(request) == 1 {
                    json!([blocker(2)]).to_string().into_bytes()
                } else {
                    b"[]".to_vec()
                }
            })
            .expect(1)
            .create(),
    );
    let dependency_write = Arc::clone(&remote);
    mocks.mocks.push(
        github
            .mock(
                "POST",
                "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            )
            .match_body(Matcher::PartialJson(json!({"issue_id": 200})))
            .with_status(201)
            .with_body_from_request(move |_| {
                dependency_write
                    .lock()
                    .expect("remote lock")
                    .dependency_present = true;
                Vec::new()
            })
            .expect(1)
            .create(),
    );
    mocks
}

#[derive(Clone, Copy)]
struct InventoryExpectations {
    labels: usize,
    issues: usize,
    comments: usize,
    dependency_inventories: usize,
}

fn mock_inventory(
    github: &mut Server,
    remote: Arc<Mutex<DraftRemote>>,
    expected: InventoryExpectations,
) -> DraftMocks {
    let mut mocks = Vec::new();
    mocks.push(
        github
            .mock("GET", "/repos/acme/widgets/labels")
            .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .expect(expected.labels)
            .create(),
    );
    mocks.push(
        github
            .mock("GET", "/repos/acme/widgets/issues/events")
            .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .expect(expected.labels)
            .create(),
    );
    let issue_inventory = Arc::clone(&remote);
    mocks.push(
        github
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
                json!(issue_inventory.lock().expect("remote lock").issues)
                    .to_string()
                    .into_bytes()
            })
            .expect(expected.issues)
            .create(),
    );
    mocks.push(
        github
            .mock("GET", "/repos/acme/widgets/issues/comments")
            .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .expect(expected.comments)
            .create(),
    );
    if expected.dependency_inventories > 0 {
        let dependencies = Arc::clone(&remote);
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
                .with_body_from_request(move |request| {
                    let remote = dependencies.lock().expect("remote lock");
                    if remote.dependency_present && path_number(request) == 1 {
                        json!([blocker(2)]).to_string().into_bytes()
                    } else {
                        b"[]".to_vec()
                    }
                })
                .expect(expected.dependency_inventories)
                .create(),
        );
    }
    DraftMocks { mocks }
}

fn remote_issue(number: u64, title: &Value, body: &Value) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": title,
        "body": body,
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

fn blocker(number: u64) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "repository_url": "https://api.github.com/repos/acme/widgets",
        "number": number,
        "state": "open"
    })
}

fn path_number(request: &Request) -> u64 {
    request
        .path()
        .split('/')
        .find_map(|segment| segment.parse::<u64>().ok())
        .expect("Issue number in path")
}

fn seed_empty_replica(github: &mut Server, state: &TempDir) {
    let remote = Arc::new(Mutex::new(DraftRemote::default()));
    let mocks = mock_inventory(
        github,
        remote,
        InventoryExpectations {
            labels: 1,
            issues: 1,
            comments: 1,
            dependency_inventories: 0,
        },
    );
    let output = grit(state, &github.url())
        .args(["sync", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("seed empty replica");
    assert_success(&output);
    mocks.assert();
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
