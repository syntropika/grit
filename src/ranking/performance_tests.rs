use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;
use crate::model::{
    BlockerIdentity, BlockerScope, Dependency, Issue, IssueIdentity, Label, LocalReplica,
};

fn fixture_outbox(
    repository: &str,
    operations: serde_json::Value,
) -> crate::outbox::PendingMutationOutbox {
    serde_json::from_value(json!({
        "schema_version": "grit.pending-mutations/v1",
        "repository": repository,
        "operations": operations,
    }))
    .expect("fixture outbox")
}

fn priority_operation(id: &str, issue_number: u64, desired: &str) -> serde_json::Value {
    json!({
        "kind": "priority_update", "id": id, "issue_number": issue_number,
        "base": {"state": "unspecified"},
        "desired": {"state": "declared", "value": desired},
    })
}

#[test]
fn cache_identity_covers_scope_parameters_and_ordered_pending_overlay() {
    let replica = repository_scale_fixture(20, 40);
    let empty = fixture_outbox(&replica.repository, json!([]));
    let working = WorkingGraph::project(&replica, &empty).expect("Working graph");
    let first_operation = priority_operation("operation-a", 1, "p1");
    let second_operation = priority_operation("operation-b", 2, "p4");
    let ordered_outbox = fixture_outbox(
        &replica.repository,
        json!([first_operation, second_operation]),
    );
    let reversed_outbox = fixture_outbox(
        &replica.repository,
        json!([second_operation, first_operation]),
    );
    let changed_outbox = fixture_outbox(
        &replica.repository,
        json!([priority_operation("operation-a", 1, "p0"), second_operation]),
    );
    let ordered_working =
        WorkingGraph::project(&replica, &ordered_outbox).expect("ordered Working graph");
    let reversed_working =
        WorkingGraph::project(&replica, &reversed_outbox).expect("reversed Working graph");
    let changed_working =
        WorkingGraph::project(&replica, &changed_outbox).expect("changed Working graph");
    let available = effective_input_hash(&working, ExecutionScope::Available);
    let assigned = effective_input_hash(&working, ExecutionScope::Assignee("ALICE"));
    let ordered = effective_input_hash(&ordered_working, ExecutionScope::Available);
    let reversed = effective_input_hash(&reversed_working, ExecutionScope::Available);
    let changed = effective_input_hash(&changed_working, ExecutionScope::Available);
    assert_ne!(available, assigned);
    assert_ne!(available, ordered);
    assert_ne!(ordered, reversed);
    assert_ne!(ordered, changed);
    assert_ne!(
        ranking_cache_key(&available, 1),
        ranking_cache_key(&available, 3)
    );

    let directory = TempDir::new().expect("temporary ranking cache");
    let mut first_cache = RankingCache::at(directory.path());
    let first = analyze_profiled(
        &ordered_working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut first_cache,
    );
    let mut reordered_cache = RankingCache::at(directory.path());
    let reordered = analyze_profiled(
        &reversed_working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut reordered_cache,
    );
    let mut changed_cache = RankingCache::at(directory.path());
    let changed = analyze_profiled(
        &changed_working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut changed_cache,
    );
    assert!(!first.profile.cache_hit);
    assert!(!reordered.profile.cache_hit);
    assert!(!changed.profile.cache_hit);
}

#[test]
fn warm_cache_preserves_the_complete_deterministic_analysis() {
    let replica = repository_scale_fixture(80, 240);
    let outbox = fixture_outbox(
        &replica.repository,
        json!([priority_operation("pending-p0", 1, "p0")]),
    );
    let working = WorkingGraph::project(&replica, &outbox).expect("Working graph");
    let directory = TempDir::new().expect("temporary ranking cache");
    let mut cold_cache = RankingCache::at(directory.path());
    let cold = analyze_profiled(
        &working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut cold_cache,
    );
    let cold_json = serde_json::to_vec(&cold.analysis).expect("serialize cold analysis");

    let mut warm_cache = RankingCache::at(directory.path());
    let warm = analyze_profiled(
        &working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut warm_cache,
    );
    let warm_json = serde_json::to_vec(&warm.analysis).expect("serialize warm analysis");

    assert!(!cold.profile.cache_hit);
    assert!(cold.profile.cache_published);
    assert!(warm.profile.cache_hit);
    assert_eq!(warm.profile.pagerank, Duration::ZERO);
    assert_eq!(warm.profile.search, Duration::ZERO);
    assert_eq!(cold_json, warm_json);
    let document: serde_json::Value =
        serde_json::from_slice(&warm_json).expect("pending cached output");
    assert_eq!(document["pending"], true);
    assert_eq!(document["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(
        document["recommendation"]["first_issue"]["priority"]["value"],
        "p0"
    );
    assert_eq!(
        document["recommendation"]["operation_ids"],
        json!(["pending-p0"])
    );
}

#[test]
fn semantically_incompatible_cache_is_rebuilt_even_with_a_valid_payload_hash() {
    let replica = repository_scale_fixture(80, 240);
    let outbox = fixture_outbox(&replica.repository, json!([]));
    let working = WorkingGraph::project(&replica, &outbox).expect("Working graph");
    let directory = TempDir::new().expect("temporary ranking cache");
    let mut cold_cache = RankingCache::at(directory.path());
    let cold = analyze_profiled(
        &working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut cold_cache,
    );
    assert!(cold.profile.cache_published);

    let cache_path = directory.path().join("ranking-next-v1-cache.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).expect("read ranking cache"))
            .expect("decode ranking cache");
    let steps = document["payload"]["search"]["candidates"][0]["steps"]
        .as_array_mut()
        .expect("cached rollout steps");
    assert!(steps.len() > 1, "fixture has a later cached rollout step");
    steps[1]["selection"] = json!("p0_ready");
    let payload = serde_json::to_vec(&document["payload"]).expect("encode altered payload");
    document["payload_hash"] = json!(hex::encode(Sha256::digest(payload)));
    std::fs::write(
        &cache_path,
        serde_json::to_vec(&document).expect("encode altered cache"),
    )
    .expect("write altered cache");

    let mut incompatible_cache = RankingCache::at(directory.path());
    let rebuilt = analyze_profiled(
        &working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut incompatible_cache,
    );
    assert!(!rebuilt.profile.cache_hit);
    assert!(rebuilt.profile.cache_published);
}

#[test]
#[ignore = "run in release mode on the documented reference hardware"]
fn repository_scale_profile_meets_the_next_v1_latency_budget() {
    const ISSUE_COUNT: usize = 5_000;
    const DEPENDENCY_COUNT: usize = 20_000;
    let replica = repository_scale_fixture(ISSUE_COUNT, DEPENDENCY_COUNT);
    let outbox = fixture_outbox(&replica.repository, json!([]));
    let working = WorkingGraph::project(&replica, &outbox).expect("Working graph");
    assert_eq!(replica.issues.len(), ISSUE_COUNT);
    assert_eq!(replica.dependencies.len(), DEPENDENCY_COUNT);
    let directory = TempDir::new().expect("temporary ranking cache");

    let mut cold_cache = RankingCache::at(directory.path());
    let cold = analyze_profiled(
        &working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut cold_cache,
    );
    let cold_serialization_started = Instant::now();
    let cold_json = serde_json::to_vec(&cold.analysis).expect("serialize cold analysis");
    let cold_serialization = cold_serialization_started.elapsed();
    let cold_total = cold.profile.total + cold_serialization;

    let mut warm_cache = RankingCache::at(directory.path());
    let warm = analyze_profiled(
        &working,
        ExecutionScope::Available,
        DEFAULT_HORIZON,
        &mut warm_cache,
    );
    let warm_serialization_started = Instant::now();
    let warm_json = serde_json::to_vec(&warm.analysis).expect("serialize warm analysis");
    let warm_serialization = warm_serialization_started.elapsed();
    let warm_total = warm.profile.total + warm_serialization;
    let cold_document: serde_json::Value =
        serde_json::from_slice(&cold_json).expect("parse analysis");
    assert_eq!(cold_document["work"]["materialized_successors"], 4_192);
    assert_eq!(cold_document["work"]["probed_successors"], 23_552);

    eprintln!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "fixture": {
                "issues": ISSUE_COUNT,
                "dependencies": DEPENDENCY_COUNT,
                "horizon": DEFAULT_HORIZON,
            },
            "unit": "microseconds",
            "cold": profile_json(cold.profile, cold_serialization, cold_total),
            "warm": profile_json(warm.profile, warm_serialization, warm_total),
            "deterministic_work": cold_document["work"].clone(),
        }))
        .expect("serialize benchmark report")
    );
    assert_eq!(cold_json, warm_json);
    assert!(warm.profile.cache_hit);
    assert!(
        cold_total < Duration::from_secs(1),
        "cold ranking took {cold_total:?}"
    );
    assert!(
        warm_total < Duration::from_millis(250),
        "warm ranking took {warm_total:?}"
    );
}

fn profile_json(
    profile: AnalysisProfile,
    serialization: Duration,
    total: Duration,
) -> serde_json::Value {
    json!({
        "graph_preparation": profile.graph_preparation.as_micros(),
        "scc_detection": profile.scc_detection.as_micros(),
        "readiness": profile.readiness.as_micros(),
        "cache_lookup": profile.cache_lookup.as_micros(),
        "pagerank": profile.pagerank.as_micros(),
        "search": profile.search.as_micros(),
        "output_assembly": profile.output_assembly.as_micros(),
        "serialization": serialization.as_micros(),
        "cache_publication": profile.cache_publication.as_micros(),
        "total": total.as_micros(),
        "cache_hit": profile.cache_hit,
        "cache_published": profile.cache_published,
    })
}

fn repository_scale_fixture(issue_count: usize, dependency_count: usize) -> LocalReplica {
    assert!(issue_count > 5);
    assert!(dependency_count.is_multiple_of(5));
    let blocked_count = dependency_count / 5;
    assert!(blocked_count < issue_count);
    let root_count = issue_count - blocked_count;
    assert!(root_count >= 7);
    let closed_root_count = (root_count / 5).max(2);
    let open_root_count = root_count - closed_root_count;
    let issues = (1..=issue_count as u64)
        .map(|number| {
            issue(
                number,
                number > open_root_count as u64 && number <= root_count as u64,
                number == issue_count as u64,
            )
        })
        .collect::<Vec<_>>();
    let mut dependencies = Vec::with_capacity(dependency_count);
    let feasible_count = (blocked_count / 400).max(1);
    for offset in 0..blocked_count {
        let blocked = root_count as u64 + offset as u64 + 1;
        let first_blocker = (offset * 5) % open_root_count;
        let blockers = if offset < feasible_count {
            vec![
                (first_blocker % open_root_count + 1) as u64,
                ((first_blocker + 1) % open_root_count + 1) as u64,
                ((first_blocker + 2) % open_root_count + 1) as u64,
                (open_root_count + 1) as u64,
                (open_root_count + 2) as u64,
            ]
        } else {
            (0..5)
                .map(|blocker_offset| {
                    ((first_blocker + blocker_offset) % open_root_count + 1) as u64
                })
                .collect()
        };
        for blocker in blockers {
            dependencies.push(dependency(blocked, blocker));
        }
    }
    LocalReplica::build_with_sync(
        "acme/repository-scale".to_owned(),
        "2026-08-07T00:00:00.000Z".to_owned(),
        Default::default(),
        Vec::new(),
        issues,
        dependencies,
    )
    .expect("valid scale fixture")
}

fn issue(number: u64, closed: bool, p0: bool) -> Issue {
    let priority = if p0 {
        "priority:p0"
    } else {
        match number % 4 {
            0 => "priority:p1",
            1 => "priority:p2",
            2 => "priority:p3",
            _ => "priority:p4",
        }
    };
    Issue {
        id: number,
        node_id: format!("I_{number}"),
        number,
        url: format!("https://github.com/acme/repository-scale/issues/{number}"),
        title: format!("Issue {number}"),
        body: String::new(),
        state: if closed { "closed" } else { "open" }.to_owned(),
        state_reason: None,
        author: None,
        assignees: Vec::new(),
        labels: vec![Label {
            id: None,
            node_id: None,
            name: priority.to_owned(),
            color: None,
            description: None,
        }],
        comments: Vec::new(),
        created_at: "2026-08-01T00:00:00Z".to_owned(),
        updated_at: "2026-08-01T00:00:00Z".to_owned(),
        closed_at: closed.then(|| "2026-08-06T00:00:00Z".to_owned()),
    }
}

fn dependency(blocked: u64, blocker: u64) -> Dependency {
    Dependency {
        blocked: IssueIdentity {
            repository: "acme/repository-scale".to_owned(),
            number: blocked,
            id: blocked,
            node_id: format!("I_{blocked}"),
        },
        blocker: BlockerIdentity {
            repository: "acme/repository-scale".to_owned(),
            number: blocker,
            state: "open".to_owned(),
            scope: BlockerScope::Internal,
            id: Some(blocker),
            node_id: Some(format!("I_{blocker}")),
        },
    }
}
