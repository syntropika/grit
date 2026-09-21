pub(super) fn empty_outbox(repository: &str) -> crate::outbox::PendingMutationOutbox {
    serde_json::from_value(serde_json::json!({
        "schema_version": "grit.pending-mutations/v1",
        "repository": repository,
        "operations": []
    }))
    .expect("empty outbox")
}

use super::*;
use crate::{
    model::{BlockerIdentity, BlockerScope, Dependency, IssueIdentity, Label, LocalReplica},
    operational::OperationalGraph,
};

#[test]
fn wide_p0_routes_complete_the_bounded_end_to_end_evaluation() {
    const ROUTE_COUNT: u64 = 100;
    let mut issues = Vec::with_capacity((ROUTE_COUNT * 2) as usize);
    let mut dependencies = Vec::with_capacity(ROUTE_COUNT as usize);
    for root in 1..=ROUTE_COUNT {
        let target = ROUTE_COUNT + root;
        issues.push(issue(root, false));
        issues.push(issue(target, true));
        dependencies.push(dependency(target, root));
    }
    let replica = LocalReplica {
        schema_version: "grit.local-replica/v1".to_owned(),
        repository: "acme/dense-p0".to_owned(),
        synced_at: "2026-08-07T00:00:00Z".to_owned(),
        input_hash: "fixture".to_owned(),
        repository_labels: Some(Vec::new()),
        issues,
        dependencies,
    };
    let graph_outbox = empty_outbox(&replica.repository);
    let graph_working = WorkingGraph::project(&replica, &graph_outbox).expect("Working graph");
    let graph = OperationalGraph::prepare(&replica);
    let result = evaluate(
        &graph_working,
        &graph,
        ExecutionScope::Available,
        None,
        3,
        8_192,
    );

    assert_eq!(result.mode, RankingMode::P0Route);
    assert_eq!(result.candidate_count, FIRST_STEP_LIMIT);
    assert_eq!(result.candidates.len(), FIRST_STEP_LIMIT);
    assert!(
        result
            .truncated_by
            .iter()
            .any(|restriction| restriction.as_str() == "p0_frontier")
    );
}

#[test]
fn horizon_one_evaluates_five_thousand_issues_without_discovery_limits() {
    let issues = (1..=5_000).map(|number| issue(number, false)).collect();
    let replica = replica(issues, Vec::new());
    let graph_outbox = empty_outbox(&replica.repository);
    let graph_working = WorkingGraph::project(&replica, &graph_outbox).expect("Working graph");
    let graph = OperationalGraph::prepare(&replica);

    let result = evaluate(
        &graph_working,
        &graph,
        ExecutionScope::Available,
        None,
        1,
        1,
    );

    assert_eq!(result.candidate_count, 5_000);
    assert_eq!(result.candidates.len(), 5_000);
    assert!(result.truncated_by.is_empty());
}

#[test]
fn multistep_search_scores_a_five_thousand_issue_frontier_with_bounded_materialization() {
    let issues = (1..=5_000).map(|number| issue(number, false)).collect();
    let replica = replica(issues, Vec::new());
    let graph_outbox = empty_outbox(&replica.repository);
    let graph_working = WorkingGraph::project(&replica, &graph_outbox).expect("Working graph");
    let graph = OperationalGraph::prepare(&replica);

    let result = evaluate(
        &graph_working,
        &graph,
        ExecutionScope::Available,
        None,
        2,
        1,
    );

    assert_eq!(result.candidate_count, FIRST_STEP_LIMIT);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].data().steps.len(), 1);
    assert_eq!(
        result
            .truncated_by
            .iter()
            .map(|restriction| restriction.as_str())
            .collect::<Vec<_>>(),
        vec!["first_step_shortlist", "probe_pool", "state_budget"]
    );
}

#[test]
fn horizon_one_does_not_report_potential_limits_for_opaque_blocked_history() {
    let issues = (1..=2_001).map(|number| issue(number, false)).collect();
    let dependencies = (2..=2_001).map(external_dependency).collect();
    let replica = replica(issues, dependencies);
    let graph_outbox = empty_outbox(&replica.repository);
    let graph_working = WorkingGraph::project(&replica, &graph_outbox).expect("Working graph");
    let graph = OperationalGraph::prepare(&replica);

    let result = evaluate(
        &graph_working,
        &graph,
        ExecutionScope::Available,
        None,
        1,
        1,
    );

    assert_eq!(result.candidate_count, 1);
    assert_eq!(result.candidates[0].data().issue.number, 1);
    assert!(result.truncated_by.is_empty());
}

#[test]
fn horizon_one_batches_shared_p0_routes_over_satisfied_history() {
    const ROUTE_COUNT: u64 = 1_000;
    let mut issues = vec![issue(1, false)];
    let mut dependencies = Vec::with_capacity((ROUTE_COUNT * 2) as usize);
    for offset in 0..ROUTE_COUNT {
        let target = 2 + offset;
        let closed_blocker = 10_000 + offset;
        issues.push(issue(target, true));
        issues.push(closed_issue(closed_blocker));
        dependencies.push(dependency(target, 1));
        dependencies.push(dependency(1, closed_blocker));
    }
    let replica = replica(issues, dependencies);
    let graph_outbox = empty_outbox(&replica.repository);
    let graph_working = WorkingGraph::project(&replica, &graph_outbox).expect("Working graph");
    let graph = OperationalGraph::prepare(&replica);

    let result = evaluate(
        &graph_working,
        &graph,
        ExecutionScope::Available,
        None,
        1,
        1,
    );

    assert_eq!(result.mode, RankingMode::P0Route);
    assert_eq!(result.candidate_count, 1);
    assert_eq!(result.candidates[0].data().issue.number, 1);
    assert_eq!(result.candidates[0].data().unlocks.len(), 1_000);
    assert!(result.truncated_by.is_empty());
}

#[test]
fn bounded_search_is_invariant_to_issue_and_dependency_permutations() {
    let mut issues = vec![
        issue(1, false),
        issue(2, false),
        issue(3, false),
        issue(10, false),
        issue(11, false),
        issue(12, false),
    ];
    let mut dependencies = vec![dependency(10, 1), dependency(11, 2), dependency(12, 10)];
    let first = replica(issues.clone(), dependencies.clone());
    issues.reverse();
    dependencies.reverse();
    let second = replica(issues, dependencies);

    let first_graph_outbox = empty_outbox(&first.repository);
    let first_graph_working =
        WorkingGraph::project(&first, &first_graph_outbox).expect("Working graph");
    let first_graph = OperationalGraph::prepare(&first);
    let second_graph_outbox = empty_outbox(&second.repository);
    let second_graph_working =
        WorkingGraph::project(&second, &second_graph_outbox).expect("Working graph");
    let second_graph = OperationalGraph::prepare(&second);
    let first_result = evaluate(
        &first_graph_working,
        &first_graph,
        ExecutionScope::Available,
        None,
        3,
        8_192,
    );
    let second_result = evaluate(
        &second_graph_working,
        &second_graph,
        ExecutionScope::Available,
        None,
        3,
        8_192,
    );

    assert_eq!(
        search_signature(&first_result),
        search_signature(&second_result)
    );
}

#[test]
fn state_budget_is_reported_in_canonical_position() {
    let issues = (1..=10).map(|number| issue(number, false)).collect();
    let replica = replica(issues, Vec::new());
    let graph_outbox = empty_outbox(&replica.repository);
    let graph_working = WorkingGraph::project(&replica, &graph_outbox).expect("Working graph");
    let graph = OperationalGraph::prepare(&replica);

    let result = evaluate(
        &graph_working,
        &graph,
        ExecutionScope::Available,
        None,
        3,
        1,
    );

    assert_eq!(result.candidate_count, 10);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(
        result
            .truncated_by
            .iter()
            .map(|restriction| restriction.as_str())
            .collect::<Vec<_>>(),
        vec!["state_budget"]
    );
}

fn replica(issues: Vec<crate::model::Issue>, dependencies: Vec<Dependency>) -> LocalReplica {
    LocalReplica {
        schema_version: "grit.local-replica/v1".to_owned(),
        repository: "acme/dense-p0".to_owned(),
        synced_at: "2026-08-07T00:00:00Z".to_owned(),
        input_hash: "fixture".to_owned(),
        repository_labels: Some(Vec::new()),
        issues,
        dependencies,
    }
}

type SearchSignature = (
    RankingMode,
    usize,
    Vec<&'static str>,
    Vec<(u64, Vec<u64>, Vec<u64>)>,
);

fn search_signature(result: &SearchResult<'_>) -> SearchSignature {
    (
        result.mode,
        result.candidate_count,
        result
            .truncated_by
            .iter()
            .map(|restriction| restriction.as_str())
            .collect(),
        result
            .candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.data().issue.number,
                    candidate
                        .data()
                        .steps
                        .iter()
                        .map(|step| step.issue.number)
                        .collect(),
                    candidate
                        .data()
                        .unlocks
                        .iter()
                        .map(|issue| issue.number)
                        .collect(),
                )
            })
            .collect(),
    )
}

fn issue(number: u64, p0: bool) -> crate::model::Issue {
    crate::model::Issue {
        id: number,
        node_id: format!("I_{number}"),
        number,
        url: format!("https://github.com/acme/dense-p0/issues/{number}"),
        title: format!("Issue {number}"),
        body: String::new(),
        state: "open".to_owned(),
        state_reason: None,
        author: None,
        assignees: Vec::new(),
        labels: p0
            .then(|| Label {
                id: None,
                node_id: None,
                name: "priority:p0".to_owned(),
                color: None,
                description: None,
            })
            .into_iter()
            .collect(),
        comments: Vec::new(),
        created_at: "2026-08-01T00:00:00Z".to_owned(),
        updated_at: "2026-08-01T00:00:00Z".to_owned(),
        closed_at: None,
    }
}

fn closed_issue(number: u64) -> crate::model::Issue {
    let mut issue = issue(number, false);
    issue.state = "closed".to_owned();
    issue.closed_at = Some("2026-08-06T00:00:00Z".to_owned());
    issue
}

fn dependency(blocked: u64, blocker: u64) -> Dependency {
    Dependency {
        blocked: IssueIdentity {
            repository: "acme/dense-p0".to_owned(),
            number: blocked,
            id: blocked,
            node_id: format!("I_{blocked}"),
        },
        blocker: BlockerIdentity {
            repository: "acme/dense-p0".to_owned(),
            number: blocker,
            state: "open".to_owned(),
            scope: BlockerScope::Internal,
            id: Some(blocker),
            node_id: Some(format!("I_{blocker}")),
        },
    }
}

fn external_dependency(blocked: u64) -> Dependency {
    Dependency {
        blocked: IssueIdentity {
            repository: "acme/dense-p0".to_owned(),
            number: blocked,
            id: blocked,
            node_id: format!("I_{blocked}"),
        },
        blocker: BlockerIdentity {
            repository: "external/private".to_owned(),
            number: blocked,
            state: "unknown".to_owned(),
            scope: BlockerScope::External,
            id: None,
            node_id: None,
        },
    }
}
