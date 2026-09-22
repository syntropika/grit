use serde_json::{Value, json};
use std::fs;

#[path = "support/scoped.rs"]
mod fixture;
use fixture::Fixture;

const REPO: &str = "acme/widgets";
const PARENT: &str = "acme/widgets#1";

#[test]
fn scope_applies_to_every_step_without_hiding_outside_blockers() {
    let fixture = Fixture::new();
    let scope = [
        "--repo",
        REPO,
        "--children-of",
        PARENT,
        "--label",
        "ready-for-agent",
    ];
    let ready = fixture.run(&[&["ready"][..], &scope].concat(), false);
    assert_eq!(
        ready["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["number"].clone())
            .collect::<Vec<_>>(),
        vec![json!(2)]
    );
    let next = fixture.run(&[&["next"][..], &scope].concat(), false);
    assert_eq!(next["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(
        next["recommendation"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| step["issue"]["number"].clone())
            .collect::<Vec<_>>(),
        vec![json!(2), json!(3)]
    );
    let unlocked: Vec<_> = next["recommendation"]["outcome"]["unlocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|outcome| outcome["issue"]["number"].clone())
        .collect();
    assert!(
        unlocked.contains(&json!(7)),
        "out-of-scope grandchildren still count as unlocked outcomes"
    );
    assert!(
        !unlocked.contains(&json!(6)),
        "outside open blocker must remain unsatisfied"
    );
    let text = serde_json::to_string(&next["recommendation"]).unwrap();
    assert!(!text.contains("Work 4"));
    let plan = fixture.run(&[&["plan"][..], &scope].concat(), false);
    assert_eq!(plan["decision"]["recommendation"], next["recommendation"]);
    assert_eq!(plan["decision"]["input_hash"], next["input_hash"]);
    assert_eq!(ready["input_hash"], next["input_hash"]);
    let site = fixture.state.path().join("graph");
    fixture.run(
        &[&["graph", "--output", site.to_str().unwrap()][..], &scope].concat(),
        false,
    );
    let artifact: Value =
        serde_json::from_slice(&fs::read(site.join("graph.json")).unwrap()).unwrap();
    assert_eq!(
        artifact["analysis"]["next"]["recommendation"],
        next["recommendation"]
    );
    assert_eq!(
        artifact["analysis"]["execution_scope"],
        next["execution_scope"]
    );
    assert!(fixture.remote.lock().unwrap().writes.is_empty());
}

#[test]
fn label_conjunction_exclusions_and_assignment_share_normalized_cache_identity() {
    let fixture = Fixture::new();
    let a = fixture.run(
        &[
            "next",
            "--repo",
            REPO,
            "--label",
            "READY-FOR-AGENT",
            "--label",
            "area:foundation",
            "--profile",
        ],
        false,
    );
    let b = fixture.run(
        &[
            "next",
            "--repo",
            REPO,
            "--label",
            "area:foundation",
            "--label",
            "ready-for-agent",
            "--label",
            "READY-FOR-AGENT",
            "--profile",
        ],
        true,
    );
    assert_eq!(a["input_hash"], b["input_hash"]);
    assert_eq!(b["performance"]["cache_hit"], true);
    let excluded = fixture.run(
        &[
            "next",
            "--repo",
            REPO,
            "--label",
            "ready-for-agent",
            "--exclude-label",
            "ready-for-agent",
        ],
        true,
    );
    assert!(excluded["recommendation"].is_null());
    assert_eq!(
        excluded["summary"]["empty_reason"],
        "ready_issues_outside_scope"
    );
    assert_ne!(a["input_hash"], excluded["input_hash"]);
    let assigned = fixture.run(
        &[
            "ready",
            "--repo",
            REPO,
            "--label",
            "area:foundation",
            "--assignee",
            "alice",
        ],
        true,
    );
    assert!(assigned["issues"].as_array().unwrap().is_empty());
}

#[test]
fn parent_inventory_survives_offline_and_refreshes_without_issue_updates() {
    let fixture = Fixture::new();
    let arguments = ["ready", "--repo", REPO, "--children-of", PARENT];
    let first = fixture.run(&arguments, false);
    let snapshot = fixture.snapshot("replica.json");
    let offline = fixture.run(&arguments, true);
    assert_eq!(first["issues"], offline["issues"]);
    assert_eq!(first["synced_at"], offline["synced_at"]);
    assert_eq!(snapshot, fixture.snapshot("replica.json"));
    fixture.remote.lock().unwrap().children.insert(1, vec![5]);
    fixture.run(&["sync", "--repo", REPO], false);
    let changed = fixture.run(&arguments, true);
    assert_eq!(changed["issues"][0]["number"], 5);
    assert_ne!(first["input_hash"], changed["input_hash"]);
}

#[test]
fn missing_or_failed_parent_inventory_never_means_an_empty_scope() {
    let fixture = Fixture::new();
    fixture.run(&["sync", "--repo", REPO], false);
    let snapshot = fixture.snapshot("replica.json");
    let missing = fixture
        .command()
        .env_remove("GH_TOKEN")
        .args(["ready", "--repo", REPO, "--children-of", PARENT, "--json"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no complete relationship inventory")
    );
    fixture
        .remote
        .lock()
        .unwrap()
        .invalid_relationship_inventory = true;
    let output = fixture
        .command()
        .args(["ready", "--repo", REPO, "--children-of", PARENT])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(snapshot, fixture.snapshot("replica.json"));
}

#[test]
fn view_reads_full_content_and_pending_changes_without_replaying_writes() {
    let fixture = Fixture::new();
    let live = fixture.run(&["view", "acme/widgets#2"], false);
    assert_eq!(live["issue"]["body"], "Specification for 2");
    assert_eq!(live["issue"]["comments"][0]["body"], "Recorded discussion");
    assert_eq!(live["issue"]["relationships"]["parent"], PARENT);
    assert_eq!(live["issue"]["blocked_by"][0]["state"], "closed");
    fixture.run(
        &[
            "update",
            "acme/widgets#2",
            "--body",
            "Revised specification",
        ],
        true,
    );
    fixture.run(
        &["comment", "acme/widgets#2", "--body", "Pending discussion"],
        true,
    );
    fixture.run(&["update", "acme/widgets#2", "--priority", "p1"], true);
    let snapshot = fixture.snapshot("replica.json");
    let outbox = fixture.snapshot("outbox.json");
    let viewed = fixture.run(&["view", "acme/widgets#2", "--offline"], false);
    assert_eq!(viewed["source"], "local");
    assert_eq!(viewed["issue"]["body"], "Revised specification");
    assert_eq!(viewed["issue"]["comments"][1]["body"], "Pending discussion");
    assert_eq!(viewed["issue"]["comments"][1]["pending"], true);
    assert_eq!(viewed["issue"]["priority"]["value"], "p1");
    assert!(
        viewed["issue"]["labels"]
            .as_array()
            .unwrap()
            .contains(&json!("priority:p1"))
    );
    assert_eq!(
        viewed["issue"]["operation_ids"].as_array().unwrap().len(),
        3
    );
    assert_eq!(snapshot, fixture.snapshot("replica.json"));
    assert_eq!(outbox, fixture.snapshot("outbox.json"));
    assert!(fixture.remote.lock().unwrap().writes.is_empty());
}

#[test]
fn pending_parent_edges_and_priority_labels_select_drafts_by_stable_identity() {
    let fixture = Fixture::new();
    fixture.run(&["sync", "--repo", REPO], false);
    let parent = fixture.run(
        &["create", "--repo", REPO, "--title", "Planning parent"],
        true,
    );
    let child = fixture.run(&["create", "--repo", REPO, "--title", "Draft child"], true);
    let parent = parent["draft"]["key"].as_str().unwrap();
    let child = child["draft"]["key"].as_str().unwrap();
    fixture.run(&["sub-issue", parent, "--add", child], true);
    let updated = fixture.run(&["update", child, "--priority", "p1"], true);
    assert!(updated["issue"].get("number").is_none());
    assert!(updated["operation"]["depends_on"].as_array().unwrap().len() == 1);
    let ready = fixture.run(
        &[
            "ready",
            "--repo",
            REPO,
            "--children-of",
            parent,
            "--label",
            "priority:p1",
        ],
        true,
    );
    assert_eq!(ready["issues"].as_array().unwrap().len(), 1);
    assert_eq!(ready["issues"][0]["key"], child);
    assert!(ready["issues"][0].get("number").is_none());
    let view = fixture.run(&["view", child, "--offline"], true);
    assert_eq!(view["issue"]["relationships"]["parent"], parent);
    fixture.run(&["sub-issue", parent, "--remove", child], true);
    let empty = fixture.run(&["ready", "--repo", REPO, "--children-of", parent], true);
    assert!(empty["issues"].as_array().unwrap().is_empty());
}

#[test]
fn draft_priorities_reconcile_in_order_and_keep_the_temporary_alias_readable() {
    let fixture = Fixture::new();
    fixture.run(&["sync", "--repo", REPO], false);
    let created = fixture.run(
        &["create", "--repo", REPO, "--title", "Prioritized draft"],
        true,
    );
    let key = created["draft"]["key"].as_str().unwrap();
    fixture.run(&["update", key, "--priority", "p1"], true);
    fixture.run(&["update", key, "--priority", "p2"], true);
    let reconciled = fixture.run(&["reconcile", "--repo", REPO], false);
    assert_eq!(reconciled["summary"]["remaining"], 0, "{reconciled}");
    let writes = fixture.remote.lock().unwrap().writes.clone();
    assert_eq!(writes, ["create:9", "label:9", "label:9", "unlabel:9"]);
    let viewed = fixture.run(&["view", key, "--offline"], true);
    assert_eq!(viewed["issue"]["number"], 9);
    assert_eq!(viewed["issue"]["priority"]["value"], "p2");
    fixture.run(&["update", key, "--priority", "none"], true);
    let cleared = fixture.run(&["view", key, "--offline"], true);
    assert_eq!(cleared["issue"]["priority"]["state"], "unspecified");
    fixture.run(&["reconcile", "--repo", REPO], false);
    assert!(
        fixture.remote.lock().unwrap().issues[&9]["labels"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ambiguous_draft_creation_blocks_priority_then_detects_a_remote_priority_conflict() {
    let fixture = Fixture::new();
    fixture.run(&["sync", "--repo", REPO], false);
    let created = fixture.run(
        &["create", "--repo", REPO, "--title", "Uncertain draft"],
        true,
    );
    let key = created["draft"]["key"].as_str().unwrap();
    fixture.run(&["update", key, "--priority", "p1"], true);
    fixture.remote.lock().unwrap().lose_create_response = true;
    let uncertain = fixture.run(&["reconcile", "--repo", REPO], false);
    assert_eq!(uncertain["summary"]["remaining"], 2);
    assert!(uncertain["operations"][1].get("issue_number").is_none());
    assert_eq!(
        uncertain["operations"][1]["temporary_id"],
        created["draft"]["temporary_id"]
    );
    assert_eq!(fixture.remote.lock().unwrap().writes, vec!["create:9"]);
    {
        let mut remote = fixture.remote.lock().unwrap();
        remote.lose_create_response = false;
        remote.issues.get_mut(&9).unwrap()["labels"] = json!(["priority:p4"]);
    }
    let recovered = fixture.run(&["reconcile", "--repo", REPO], false);
    assert_eq!(recovered["summary"]["remaining"], 1);
    assert_eq!(recovered["summary"]["conflicting"], 1);
    assert_eq!(recovered["operations"][1]["remote"]["value"], "p4");
    assert_eq!(fixture.remote.lock().unwrap().writes, vec!["create:9"]);
}
