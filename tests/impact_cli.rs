use serde_json::{Value, json};
use std::fs;

#[path = "support/scoped.rs"]
mod fixture;
use fixture::Fixture;

const REPO: &str = "acme/widgets";

fn diamond() -> Fixture {
    let fixture = Fixture::new();
    {
        let mut remote = fixture.remote.lock().unwrap();
        remote.blockers = [
            (2, vec![1]),
            (3, vec![1]),
            (4, vec![2, 3]),
            (5, vec![1, 6]),
            (7, vec![4]),
            (8, vec![1]),
        ]
        .into();
        remote.issues.get_mut(&1).unwrap()["assignees"] =
            json!([{"id": 2, "node_id": "U_2", "login": "alice"}]);
        remote.issues.insert(9, fixture::issue(9, &[]));
        remote.blockers.insert(9, vec![8]);
    }
    fixture
}

#[test]
fn impact_deduplicates_diamonds_respects_and_and_matches_the_scoped_private_graph() {
    let fixture = diamond();
    let view = fixture.run(&["view", "acme/widgets#1"], false);
    let impact = &view["issue"]["impact"];
    assert_eq!(
        impact["ready_now"], true,
        "assignment does not change readiness"
    );
    assert_eq!(impact["direct_dependents"], 3);
    assert_eq!(impact["downstream"], json!({"count":5,"complete":true}));
    assert_eq!(
        impact["immediate_unlocks"],
        json!({"count":2,"examples":["acme/widgets#2","acme/widgets#3"]})
    );
    assert_eq!(
        impact["still_blocked"]["total"],
        json!({"count":3,"complete":true})
    );
    assert_eq!(
        impact["still_blocked"]["examples"][0]["blockers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        impact["still_blocked"]["examples"][1]["blockers"][0]["key"],
        "acme/widgets#6"
    );
    assert_eq!(impact["chain_depth"], json!({"state":"finite","edges":3}));

    let snapshot = fixture.snapshot("replica.json");
    let site = fixture.state.path().join("impact-graph");
    fixture.run(
        &[
            "graph",
            "--repo",
            REPO,
            "--label",
            "ready-for-agent",
            "--output",
            site.to_str().unwrap(),
        ],
        true,
    );
    let graph: Value = serde_json::from_slice(&fs::read(site.join("graph.json")).unwrap()).unwrap();
    assert_eq!(graph["schema_version"], "hyfa.graph-artifact/v4");
    let node = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["common"]["number"] == 1)
        .unwrap();
    assert_eq!(
        node["impact"], *impact,
        "impact is independent of execution scope"
    );
    let next = fixture.run(
        &["next", "--repo", REPO, "--label", "ready-for-agent"],
        true,
    );
    assert_eq!(
        graph["analysis"]["next"]["recommendation"],
        next["recommendation"]
    );
    let closed = fixture.run(&["view", "acme/widgets#8", "--offline"], true);
    assert!(closed["issue"]["impact"].is_null());
    assert_eq!(snapshot, fixture.snapshot("replica.json"));
    assert!(fixture.remote.lock().unwrap().writes.is_empty());
}

#[test]
fn cycles_and_blocked_subjects_never_claim_executable_immediate_outcomes() {
    let fixture = Fixture::new();
    fixture.remote.lock().unwrap().blockers = [(2, vec![1, 3]), (3, vec![2]), (4, vec![4])].into();
    fixture.run(&["sync", "--repo", REPO], false);
    let root = fixture.run(&["view", "acme/widgets#1", "--offline"], true);
    assert_eq!(root["issue"]["impact"]["downstream"]["count"], 2);
    assert_eq!(
        root["issue"]["impact"]["chain_depth"]["state"],
        "unavailable"
    );
    assert_eq!(root["issue"]["impact"]["immediate_unlocks"]["count"], 0);
    assert_eq!(
        root["issue"]["impact"]["still_blocked"]["examples"][0]["cyclic"],
        true
    );
    let cyclic = fixture.run(&["view", "acme/widgets#2", "--offline"], true);
    assert_eq!(
        cyclic["issue"]["impact"]["downstream"]["count"], 1,
        "exclude the subject itself"
    );
    assert_eq!(cyclic["issue"]["impact"]["ready_now"], false);
    assert!(cyclic["issue"]["impact"]["immediate_unlocks"].is_null());
    assert!(cyclic["issue"]["impact"]["still_blocked"].is_null());
    let self_loop = fixture.run(&["view", "acme/widgets#4", "--offline"], true);
    assert_eq!(self_loop["issue"]["impact"]["downstream"]["count"], 0);
    assert_eq!(self_loop["issue"]["impact"]["direct_dependents"], 0);
    assert_eq!(
        self_loop["issue"]["impact"]["chain_depth"]["state"],
        "unavailable"
    );
}

#[test]
fn external_open_and_unknown_blockers_remain_explained_after_completion() {
    let fixture = Fixture::new();
    {
        let mut remote = fixture.remote.lock().unwrap();
        remote.blockers = [(2, vec![1]), (3, vec![1]), (4, vec![1])].into();
        for (number, state) in [(2, "open"), (3, "unknown"), (4, "closed")] {
            let mut blocker = fixture::issue(99, &[]);
            blocker["repository_url"] = json!("https://api.github.com/repos/outside/team");
            blocker["html_url"] = json!("https://github.com/outside/team/issues/99");
            blocker["state"] = json!(state);
            remote.external_blockers.insert(number, vec![blocker]);
        }
    }
    let view = fixture.run(&["view", "acme/widgets#1"], false);
    let impact = &view["issue"]["impact"];
    assert_eq!(
        impact["immediate_unlocks"]["examples"],
        json!(["acme/widgets#4"])
    );
    let blocked = impact["still_blocked"]["examples"].as_array().unwrap();
    assert_eq!(blocked.len(), 2);
    assert_eq!(
        blocked[0]["blockers"][0],
        json!({"key":"outside/team#99","external":true,"unknown":false})
    );
    assert_eq!(blocked[1]["blockers"][0]["unknown"], true);
}

#[test]
fn pending_drafts_closures_and_edge_edits_change_impact_without_remote_writes() {
    let fixture = diamond();
    fixture.run(&["sync", "--repo", REPO], false);
    let snapshot = fixture.snapshot("replica.json");
    fixture.run(&["update", "acme/widgets#6", "--state", "closed"], true);
    let draft = fixture.run(
        &["create", "--repo", REPO, "--title", "Pending dependent"],
        true,
    );
    let key = draft["draft"]["key"].as_str().unwrap();
    fixture.run(&["block", key, "--by", "acme/widgets#1"], true);
    let outbox = fixture.snapshot("outbox.json");
    let view = fixture.run(&["view", "acme/widgets#1", "--offline"], true);
    let impact = &view["issue"]["impact"];
    assert_eq!(impact["pending"], true);
    assert_eq!(impact["immediate_unlocks"]["count"], 4);
    assert!(
        impact["immediate_unlocks"]["examples"]
            .as_array()
            .unwrap()
            .contains(&json!(key))
    );
    assert_eq!(impact["downstream"]["count"], 6);
    assert_eq!(snapshot, fixture.snapshot("replica.json"));
    assert_eq!(outbox, fixture.snapshot("outbox.json"));
    fixture.run(&["unblock", key, "--by", "acme/widgets#1"], true);
    let changed = fixture.run(&["view", "acme/widgets#1", "--offline"], true);
    assert_eq!(changed["issue"]["impact"]["downstream"]["count"], 5);
    assert!(fixture.remote.lock().unwrap().writes.is_empty());
}
