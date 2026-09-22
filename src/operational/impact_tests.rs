use super::*;
use crate::{
    model::{Dependency, Issue, LocalReplica, REPLICA_SCHEMA_VERSION},
    outbox::PendingMutationOutbox,
    working_graph::WorkingGraph,
};
use serde_json::{Value, json};

fn replica(count: u64, edges: &[(u64, u64)]) -> LocalReplica {
    let issues: Vec<Issue> = (1..=count)
        .map(|number| {
            serde_json::from_value(json!({
        "id": number, "node_id": format!("I_{number}"), "number":number,
        "url":format!("https://github.com/acme/widgets/issues/{number}"),
        "title":format!("Issue {number}"), "body":"", "state":"open", "state_reason":null,
        "author":null, "assignees":[], "labels":[], "comments":[],
        "created_at":"2026-09-22T00:00:00Z", "updated_at":"2026-09-22T00:00:00Z", "closed_at":null
    })).unwrap()
        })
        .collect();
    let dependencies: Vec<Dependency> = edges.iter().map(|(blocked, blocker)| serde_json::from_value(json!({
        "blocked":{"repository":"acme/widgets", "number":blocked, "id":blocked, "node_id":format!("I_{blocked}")},
        "blocker":{"repository":"acme/widgets", "number":blocker, "scope":"internal", "state":"open", "id":blocker, "node_id":format!("I_{blocker}")}
    })).unwrap()).collect();
    LocalReplica {
        schema_version: REPLICA_SCHEMA_VERSION.to_owned(),
        repository: "acme/widgets".to_owned(),
        synced_at: "2026-09-22T00:00:00Z".to_owned(),
        input_hash: "fixture".to_owned(),
        repository_labels: Some(Vec::new()),
        issues,
        dependencies,
        relationships: Default::default(),
        sync: Default::default(),
    }
}

fn outbox() -> PendingMutationOutbox {
    serde_json::from_value(json!({"schema_version":"hyfa.pending-mutations/v1","repository":"acme/widgets","operations":[]})).unwrap()
}

#[test]
fn truncated_reach_is_a_lower_bound_while_depth_and_immediate_outcomes_stay_exact() {
    let replica = replica(20, &(2..=20).map(|n| (n, n - 1)).collect::<Vec<_>>());
    let outbox = outbox();
    let working = WorkingGraph::project(&replica, &outbox).unwrap();
    let prepared = PreparedRepository::prepare(&working);
    let analysis = ImpactAnalysis::prepare(&prepared);
    let short = analysis.analyze(1, 5).unwrap();
    assert!(!short.downstream.complete);
    assert_eq!(short.downstream.count, 2);
    assert_eq!(short.immediate_unlocks.as_ref().unwrap().count, 1);
    assert_eq!(short.still_blocked.as_ref().unwrap().total.count, 1);
    assert!(matches!(
        short.chain_depth,
        ChainDepth::Finite { edges: 19 }
    ));
    let empty = analysis.analyze(1, 0).unwrap();
    assert_eq!(empty.downstream.count, 0);
    assert_eq!(empty.still_blocked.unwrap().total.count, 0);
    let full = analysis.for_issue(1).unwrap();
    assert!(full.downstream.complete);
    assert_eq!(full.downstream.count, 19);
}

#[test]
fn examples_are_bounded_independently_of_exact_counts_and_all_remaining_blockers() {
    let mut edges: Vec<_> = (2..=12).map(|n| (n, 1)).collect();
    edges.extend((2..=8).map(|n| (12, n)));
    let replica = replica(12, &edges);
    let outbox = outbox();
    let working = WorkingGraph::project(&replica, &outbox).unwrap();
    let prepared = PreparedRepository::prepare(&working);
    let impact = ImpactAnalysis::prepare(&prepared).for_issue(1).unwrap();
    let unlocks = impact.immediate_unlocks.unwrap();
    assert_eq!(unlocks.count, 10);
    assert_eq!(unlocks.examples.len(), EXAMPLE_LIMIT);
    let blocked = &impact.still_blocked.unwrap().examples[0];
    assert_eq!(blocked.blocker_count, 7);
    assert_eq!(blocked.blockers.len(), EXAMPLE_LIMIT);
}

#[test]
fn unknown_internal_endpoints_are_distinct_and_never_exposed_as_synthetic_issue_numbers() {
    let replica = replica(2, &[(2, 1), (2, u64::MAX), (2, u64::MAX - 1)]);
    let outbox = outbox();
    let working = WorkingGraph::project(&replica, &outbox).unwrap();
    let prepared = PreparedRepository::prepare(&working);
    let impact = ImpactAnalysis::prepare(&prepared).for_issue(1).unwrap();
    assert_eq!(impact.immediate_unlocks.unwrap().count, 0);
    let blocked = &impact.still_blocked.unwrap().examples[0];
    assert_eq!(blocked.blocker_count, 2);
    assert!(
        blocked
            .blockers
            .iter()
            .all(|blocker| blocker.unknown && blocker.key.is_none())
    );
}

#[test]
fn impact_is_deterministic_across_input_order() {
    let mut replica = replica(
        8,
        &[
            (2, 1),
            (3, 1),
            (4, 2),
            (4, 3),
            (5, 4),
            (6, 5),
            (7, 6),
            (8, 7),
        ],
    );
    let outbox = outbox();
    let read = |replica: &LocalReplica| -> Value {
        let working = WorkingGraph::project(replica, &outbox).unwrap();
        let prepared = PreparedRepository::prepare(&working);
        serde_json::to_value(ImpactAnalysis::prepare(&prepared).for_issue(1)).unwrap()
    };
    let original = read(&replica);
    replica.issues.reverse();
    replica.dependencies.reverse();
    assert_eq!(original, read(&replica));
}

#[test]
#[ignore = "optimized whole-graph impact benchmark; run with --release"]
fn repository_scale_impact_stays_bounded() {
    let edges: Vec<_> = (2..=5000)
        .flat_map(|n| {
            (1..=4)
                .filter(move |distance| n > *distance)
                .map(move |distance| (n, n - distance))
        })
        .collect();
    let replica = replica(5000, &edges);
    let outbox = outbox();
    let working = WorkingGraph::project(&replica, &outbox).unwrap();
    let prepared = PreparedRepository::prepare(&working);
    let started = std::time::Instant::now();
    let analysis = ImpactAnalysis::prepare(&prepared);
    let mut summaries = Vec::new();
    for number in 1..=5000 {
        summaries.push(analysis.for_issue(number).unwrap());
    }
    let bytes = serde_json::to_vec(&summaries).unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "5000 Issue impact reports: {elapsed:?}, {} serialized bytes",
        bytes.len()
    );
    assert!(elapsed < std::time::Duration::from_secs(3));
    assert!(bytes.len() < 8_000_000);
}
