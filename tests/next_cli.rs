use std::process::Command;

use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn next_evaluates_the_complete_frontier_with_and_unlocks_and_deduplication() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p3"], &[]),
        issue(2, "open", &["priority:p1"], &[]),
        issue(3, "open", &[], &[]),
        issue(4, "open", &["priority:p4"], &[]),
        issue(5, "open", &["priority:p1", "priority:p4"], &[]),
        issue(10, "open", &["priority:p1"], &[]),
        issue(11, "open", &["priority:p4"], &[]),
        issue(12, "open", &["priority:p3"], &[]),
        issue(13, "open", &["priority:p1"], &["alice"]),
        issue(14, "closed", &["priority:p0"], &[]),
        issue(15, "open", &["priority:p0"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![]),
        (4, vec![]),
        (5, vec![]),
        (
            10,
            vec![internal_blocker(1, "open"), internal_blocker(2, "open")],
        ),
        (
            11,
            vec![internal_blocker(1, "open"), internal_blocker(1, "open")],
        ),
        (12, vec![internal_blocker(1, "open")]),
        (13, vec![internal_blocker(1, "open")]),
        (14, vec![internal_blocker(2, "open")]),
        (
            15,
            vec![internal_blocker(11, "open"), internal_blocker(12, "open")],
        ),
    ];
    let mocks = mock_repository(&mut github, "acme/next", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/next", true)
        .output()
        .expect("run grit next");
    assert_success(&output);
    let next: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(next["schema_version"], "grit.next/v1");
    assert_eq!(next["policy_version"], "next/v1");
    assert_eq!(next["parameters"]["horizon"], 1);
    assert_eq!(next["mode"], "normal");
    assert_eq!(next["search_complete"], true);
    assert_eq!(next["truncated_by"], json!([]));
    assert_eq!(next["global_optimum_claimed"], true);
    assert_eq!(next["runner_up_scope"], "global");
    assert_ne!(next["replica_snapshot_hash"], Value::Null);
    assert_ne!(next["input_hash"], next["replica_snapshot_hash"]);
    assert_eq!(next["metrics"]["unlock_profile"]["state"], "available");
    assert_eq!(next["metrics"]["pagerank"]["state"], "available");

    let recommendation = &next["recommendation"];
    assert_eq!(recommendation["first_issue"]["number"], 1);
    assert_eq!(recommendation["rollout"]["steps"][0]["mode"], "normal");
    assert_eq!(recommendation["outcome"]["unlock_profile"]["count"], 3);
    assert_eq!(
        recommendation["outcome"]["unlock_profile"]["priority_profile"],
        json!({"p1": 1, "neutral": 0, "p3": 1, "p4": 1})
    );
    assert_eq!(
        recommendation["outcome"]["unlocks"]
            .as_array()
            .expect("unlocks")
            .iter()
            .map(|unlock| unlock["issue"]["number"].as_u64().expect("Issue number"))
            .collect::<Vec<_>>(),
        vec![11, 12, 13]
    );
    assert_eq!(
        recommendation["outcome"]["unlock_availability"],
        json!({"available": 2, "assigned": 1})
    );
    assert_eq!(
        next["alternatives"]
            .as_array()
            .expect("alternatives")
            .iter()
            .map(|alternative| alternative["first_issue"]["number"]
                .as_u64()
                .expect("Issue number"))
            .collect::<Vec<_>>(),
        vec![2, 3, 5, 4]
    );
    assert_eq!(
        next["comparison_to_runner_up"]["reason_code"],
        "unlocks_more_work"
    );
    assert_eq!(next["comparison_to_runner_up"]["runner_up"]["number"], 2);
    assert_eq!(next["close_call"], false);
    assert_eq!(next["warnings"][0]["code"], "priority_conflict");
    assert_eq!(next["warnings"][0]["issue_number"], 5);
    mocks.assert();

    let ready = ready_command(&state, &github.url(), "acme/next")
        .output()
        .expect("offline grit ready");
    assert_success(&ready);
    let ready: Value = serde_json::from_slice(&ready.stdout).expect("ready JSON");
    let ready_numbers = ready["issues"]
        .as_array()
        .expect("ready Issues")
        .iter()
        .map(|issue| issue["number"].as_u64().expect("Issue number"))
        .collect::<Vec<_>>();
    let mut ranked_numbers = vec![
        next["recommendation"]["first_issue"]["number"]
            .as_u64()
            .expect("recommendation number"),
    ];
    ranked_numbers.extend(
        next["alternatives"]
            .as_array()
            .expect("alternatives")
            .iter()
            .map(|alternative| {
                alternative["first_issue"]["number"]
                    .as_u64()
                    .expect("alternative number")
            }),
    );
    ranked_numbers.sort_unstable();
    assert_eq!(ranked_numbers, ready_numbers);
}

#[test]
fn default_rollout_follows_executable_chains_and_counts_distinct_ready_transitions() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mut issues = vec![
        issue(1, "open", &["priority:p4"], &[]),
        issue(2, "open", &["priority:p1"], &[]),
        issue(10, "open", &["priority:p4"], &[]),
        issue(11, "open", &["priority:p4"], &[]),
        issue(17, "open", &["priority:p1"], &[]),
        issue(20, "open", &["priority:p1"], &[]),
        issue(21, "open", &["priority:p1"], &[]),
    ];
    for number in 12..=16 {
        let assignees = if number == 12 { &["alice"][..] } else { &[] };
        issues.push(issue(number, "open", &["priority:p4"], assignees));
    }
    let mut fanout_dependencies = vec![
        internal_blocker(1, "open"),
        internal_blocker(10, "open"),
        internal_blocker(11, "open"),
        internal_blocker(11, "open"),
    ];
    let mut dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (10, vec![internal_blocker(1, "open")]),
        (11, vec![internal_blocker(10, "open")]),
        (12, std::mem::take(&mut fanout_dependencies)),
        (17, vec![internal_blocker(12, "open")]),
        (20, vec![internal_blocker(2, "open")]),
        (21, vec![internal_blocker(2, "open")]),
    ];
    for number in 13..=16 {
        dependencies.push((number, vec![internal_blocker(11, "open")]));
    }
    let mocks = mock_repository(&mut github, "acme/rollout", issues, dependencies);

    let output = next_default_command(&state, &github.url(), "acme/rollout", true)
        .output()
        .expect("run three-step next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    let recommendation = &output["recommendation"];

    assert_eq!(output["parameters"]["horizon"], 3);
    assert_eq!(output["mode"], "normal");
    assert_eq!(output["search_complete"], true);
    assert_eq!(recommendation["first_issue"]["number"], 1);
    assert_eq!(
        rollout_numbers(recommendation),
        vec![1, 10, 11],
        "every continuation must come from the newly produced Executable state"
    );
    assert_eq!(
        recommendation["outcome"]["unlock_profile"],
        json!({
            "count": 7,
            "priority_profile": {"p1": 0, "neutral": 0, "p3": 0, "p4": 7},
            "curve": [1, 2, 7],
            "p0_curve": [0, 0, 0],
            "step_priorities": ["p4", "p4", "p4"]
        })
    );
    assert_eq!(
        unlock_numbers(recommendation),
        vec![10, 11, 12, 13, 14, 15, 16]
    );
    assert_eq!(
        recommendation["outcome"]["unlock_availability"],
        json!({"available": 6, "assigned": 1})
    );
    assert!(!unlock_numbers(recommendation).contains(&17));
    assert_eq!(
        output["comparison_to_runner_up"]["reason_code"],
        "unlocks_more_work"
    );
    mocks.assert();
}

#[test]
fn normal_rollouts_compare_downstream_priority_before_unlock_timing() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p4"], &[]),
        issue(2, "open", &["priority:p4"], &[]),
        issue(10, "open", &["priority:p4"], &[]),
        issue(11, "open", &["priority:p4"], &[]),
        issue(12, "open", &["priority:p4"], &[]),
        issue(13, "open", &["priority:p4"], &[]),
        issue(20, "open", &["priority:p1"], &[]),
        issue(21, "open", &["priority:p1"], &[]),
        issue(22, "open", &["priority:p1"], &[]),
        issue(23, "open", &["priority:p1"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (10, vec![internal_blocker(1, "open")]),
        (11, vec![internal_blocker(10, "open")]),
        (12, vec![internal_blocker(11, "open")]),
        (13, vec![internal_blocker(11, "open")]),
        (20, vec![internal_blocker(2, "open")]),
        (21, vec![internal_blocker(20, "open")]),
        (22, vec![internal_blocker(21, "open")]),
        (23, vec![internal_blocker(21, "open")]),
    ];
    let mocks = mock_repository(&mut github, "acme/profile", issues, dependencies);

    let output = next_default_command(&state, &github.url(), "acme/profile", true)
        .output()
        .expect("run Priority-profile next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(
        output["recommendation"]["outcome"]["unlock_profile"]["count"],
        4
    );
    assert_eq!(
        output["recommendation"]["outcome"]["unlock_profile"]["priority_profile"],
        json!({"p1": 4, "neutral": 0, "p3": 0, "p4": 0})
    );
    assert_eq!(
        output["comparison_to_runner_up"]["reason_code"],
        "unlocks_higher_priority_work"
    );
    mocks.assert();
}

#[test]
fn normal_rollouts_compare_unlock_curve_before_step_priority_sequence() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p4"], &[]),
        issue(2, "open", &["priority:p4"], &[]),
        issue(10, "open", &["priority:p4"], &[]),
        issue(11, "open", &["priority:p4"], &[]),
        issue(12, "open", &["priority:p4"], &[]),
        issue(13, "open", &["priority:p4"], &[]),
        issue(20, "open", &["priority:p4"], &[]),
        issue(21, "open", &["priority:p4"], &[]),
        issue(22, "open", &["priority:p4"], &[]),
        issue(23, "open", &["priority:p4"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (10, vec![internal_blocker(1, "open")]),
        (11, vec![internal_blocker(10, "open")]),
        (12, vec![internal_blocker(11, "open")]),
        (13, vec![internal_blocker(11, "open")]),
        (20, vec![internal_blocker(2, "open")]),
        (21, vec![internal_blocker(2, "open")]),
        (22, vec![internal_blocker(20, "open")]),
        (23, vec![internal_blocker(22, "open")]),
    ];
    let mocks = mock_repository(&mut github, "acme/curve", issues, dependencies);

    let output = next_default_command(&state, &github.url(), "acme/curve", true)
        .output()
        .expect("run Unlock-curve next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(
        output["recommendation"]["outcome"]["unlock_profile"]["curve"],
        json!([2, 3, 4])
    );
    assert_eq!(
        output["alternatives"][0]["outcome"]["unlock_profile"]["curve"],
        json!([1, 3, 4])
    );
    assert_eq!(
        output["comparison_to_runner_up"]["reason_code"],
        "unlocks_earlier"
    );
    mocks.assert();
}

#[test]
fn normal_rollouts_compare_and_pad_the_step_priority_sequence() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p1"], &[]),
        issue(2, "open", &["priority:p4"], &[]),
        issue(10, "open", &["priority:p4"], &[]),
        issue(11, "open", &["priority:p4"], &[]),
        issue(12, "open", &["priority:p4"], &[]),
        issue(13, "open", &["priority:p4"], &[]),
        issue(20, "open", &["priority:p4"], &[]),
        issue(21, "open", &["priority:p4"], &[]),
        issue(22, "open", &["priority:p4"], &[]),
        issue(23, "open", &["priority:p4"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (10, vec![internal_blocker(1, "open")]),
        (11, vec![internal_blocker(10, "open")]),
        (12, vec![internal_blocker(11, "open")]),
        (13, vec![internal_blocker(11, "open")]),
        (20, vec![internal_blocker(2, "open")]),
        (21, vec![internal_blocker(20, "open")]),
        (22, vec![internal_blocker(21, "open")]),
        (23, vec![internal_blocker(21, "open")]),
    ];
    let mocks = mock_repository(&mut github, "acme/steps", issues, dependencies);

    let output = next_default_command(&state, &github.url(), "acme/steps", true)
        .output()
        .expect("run step-Priority next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(
        output["recommendation"]["outcome"]["unlock_profile"]["step_priorities"],
        json!(["p1", "p4", "p4"])
    );
    assert_eq!(
        output["comparison_to_runner_up"]["reason_code"],
        "declared_priority_tiebreak"
    );
    mocks.assert();

    drop(github);
    let human = next_default_human_command(&state, "http://127.0.0.1:9", "acme/steps")
        .output()
        .expect("run human step-Priority output");
    assert_success(&human);
    assert!(
        String::from_utf8_lossy(&human.stdout)
            .contains("the rollout's step-Priority sequence breaks the tie")
    );

    let mut padding_github = Server::new();
    let padding_state = TempDir::new().expect("padding state directory");
    let padding_mocks = mock_repository(
        &mut padding_github,
        "acme/padding",
        vec![issue(1, "open", &["priority:p1"], &[])],
        vec![(1, vec![])],
    );
    let padding = next_default_command(&padding_state, &padding_github.url(), "acme/padding", true)
        .output()
        .expect("run padded rollout");
    assert_success(&padding);
    let padding: Value = serde_json::from_slice(&padding.stdout).expect("padding JSON");
    assert_eq!(
        padding["recommendation"]["outcome"]["unlock_profile"]["curve"],
        json!([0, 0, 0])
    );
    assert_eq!(
        padding["recommendation"]["outcome"]["unlock_profile"]["step_priorities"],
        json!(["p1", "no_step", "no_step"])
    );
    padding_mocks.assert();
}

#[test]
fn state_budget_restriction_is_reported_without_a_global_optimum_claim() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = (1..=30)
        .map(|number| {
            let priority = if number == 1 {
                "priority:p1"
            } else {
                "priority:p4"
            };
            issue(number, "open", &[priority], &[])
        })
        .collect();
    let dependencies = (1..=30).map(|number| (number, vec![])).collect();
    let mocks = mock_repository(&mut github, "acme/truncated", issues, dependencies);

    let output = next_default_command(&state, &github.url(), "acme/truncated", true)
        .output()
        .expect("run bounded rollout search");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["summary"]["candidate_count"], 30);
    assert_eq!(output["search_complete"], false);
    assert_eq!(output["truncated_by"], json!(["state_budget"]));
    assert_eq!(output["global_optimum_claimed"], false);
    assert_eq!(output["runner_up_scope"], "explored");
    mocks.assert();

    drop(github);
    let human = next_default_human_command(&state, "http://127.0.0.1:9", "acme/truncated")
        .output()
        .expect("run truncated human output");
    assert_success(&human);
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(
        stderr.contains("restricted by state_budget"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("no global optimum is claimed"),
        "stderr: {stderr}"
    );
}

#[test]
fn deferred_multistep_p0_routes_do_not_make_an_unsupported_optimum_claim() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p4"], &[]),
        issue(2, "open", &["priority:p4"], &[]),
        issue(3, "open", &["priority:p0"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![internal_blocker(1, "open")]),
        (3, vec![internal_blocker(2, "open")]),
    ];
    let mocks = mock_repository(&mut github, "acme/deferred-p0", issues, dependencies);

    let output = next_default_command(&state, &github.url(), "acme/deferred-p0", true)
        .output()
        .expect("run deferred P0 search");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["search_complete"], false);
    assert_eq!(output["truncated_by"], json!(["p0_frontier"]));
    assert_eq!(output["global_optimum_claimed"], false);
    assert_eq!(output["runner_up_scope"], "explored");
    mocks.assert();
}

#[test]
fn executable_p0_gate_prefers_the_p0_that_unlocks_more_critical_work() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &["priority:p0"], &[]),
        issue(2, "open", &["priority:p0"], &[]),
        issue(3, "open", &["priority:p1"], &[]),
        issue(10, "open", &["priority:p4"], &[]),
        issue(11, "open", &["priority:p0"], &[]),
        issue(20, "open", &["priority:p1"], &[]),
        issue(21, "open", &["priority:p1"], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![]),
        (10, vec![internal_blocker(1, "open")]),
        (11, vec![internal_blocker(2, "open")]),
        (20, vec![internal_blocker(3, "open")]),
        (21, vec![internal_blocker(3, "open")]),
    ];
    let mocks = mock_repository(&mut github, "acme/p0-ready", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/p0-ready", true)
        .output()
        .expect("run P0 next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(output["mode"], "p0_ready");
    assert_eq!(output["recommendation"]["first_issue"]["number"], 2);
    assert_eq!(
        output["recommendation"]["outcome"]["unlock_profile"]["p0_curve"],
        json!([1])
    );
    assert_eq!(
        output["alternatives"]
            .as_array()
            .expect("alternatives")
            .len(),
        1
    );
    assert_eq!(output["alternatives"][0]["first_issue"]["number"], 1);
    assert!(reason_codes(&output).contains(&"ready_p0"));
    assert!(reason_codes(&output).contains(&"unlocks_more_p0"));
    mocks.assert();
}

#[test]
fn one_step_p0_route_beats_a_larger_noncritical_fanout() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mut issues = vec![
        issue(1, "open", &["priority:p4"], &[]),
        issue(2, "open", &["priority:p1"], &[]),
        issue(10, "open", &["priority:p0"], &[]),
    ];
    let mut dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (10, vec![internal_blocker(1, "open")]),
    ];
    for number in 20..26 {
        issues.push(issue(number, "open", &["priority:p1"], &[]));
        dependencies.push((number, vec![internal_blocker(2, "open")]));
    }
    let mocks = mock_repository(&mut github, "acme/p0-route", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/p0-route", true)
        .output()
        .expect("run routed P0 next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(output["mode"], "p0_route");
    assert_eq!(output["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(output["alternatives"], json!([]));
    assert!(reason_codes(&output).contains(&"shortest_p0_route"));
    mocks.assert();
}

#[test]
fn pagerank_then_stable_node_key_break_only_structural_ties() {
    let state = TempDir::new().expect("temporary state directory");
    let mut pagerank_github = Server::new();
    let issues = vec![
        issue(1, "open", &[], &[]),
        issue(2, "open", &[], &[]),
        issue(3, "open", &[], &[]),
        issue(10, "open", &[], &[]),
        issue(11, "open", &[], &[]),
        issue(12, "open", &[], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![]),
        (3, vec![external_blocker("partners/api", 90, "unknown")]),
        (
            10,
            vec![internal_blocker(1, "open"), internal_blocker(3, "open")],
        ),
        (
            11,
            vec![internal_blocker(1, "open"), internal_blocker(3, "open")],
        ),
        (
            12,
            vec![internal_blocker(2, "open"), internal_blocker(3, "open")],
        ),
    ];
    let mocks = mock_repository(&mut pagerank_github, "acme/pagerank", issues, dependencies);
    let pagerank = next_command(&state, &pagerank_github.url(), "acme/pagerank", true)
        .output()
        .expect("run PageRank next");
    assert_success(&pagerank);
    let pagerank: Value = serde_json::from_slice(&pagerank.stdout).expect("next JSON");
    assert_eq!(pagerank["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(
        pagerank["comparison_to_runner_up"]["reason_code"],
        "pagerank_tiebreak"
    );
    assert_eq!(pagerank["close_call"], true);
    assert!(
        pagerank["recommendation"]["pagerank_bucket"]
            .as_u64()
            .expect("winner bucket")
            > pagerank["alternatives"][0]["pagerank_bucket"]
                .as_u64()
                .expect("runner-up bucket")
    );
    mocks.assert();

    let mut stable_github = Server::new();
    let stable_state = TempDir::new().expect("stable-key state directory");
    let stable_mocks = mock_repository(
        &mut stable_github,
        "acme/stable",
        vec![issue(2, "open", &[], &[]), issue(1, "open", &[], &[])],
        vec![(2, vec![]), (1, vec![])],
    );
    let stable = next_command(&stable_state, &stable_github.url(), "acme/stable", true)
        .output()
        .expect("run stable next");
    assert_success(&stable);
    let stable: Value = serde_json::from_slice(&stable.stdout).expect("next JSON");
    assert_eq!(stable["recommendation"]["first_issue"]["number"], 1);
    assert_eq!(
        stable["comparison_to_runner_up"]["reason_code"],
        "deterministic_tiebreak"
    );
    assert_eq!(stable["close_call"], true);
    stable_mocks.assert();
}

#[test]
fn bounded_output_keeps_a_late_winner_and_the_global_runner_up() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let issues = (1..=13)
        .map(|number| {
            let priority = if number == 13 {
                "priority:p1"
            } else {
                "priority:p4"
            };
            issue(number, "open", &[priority], &[])
        })
        .collect();
    let dependencies = (1..=13).map(|number| (number, vec![])).collect();
    let mocks = mock_repository(&mut github, "acme/bounded", issues, dependencies);

    let output = next_command(&state, &github.url(), "acme/bounded", true)
        .output()
        .expect("run bounded next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");

    assert_eq!(output["summary"]["candidate_count"], 13);
    assert_eq!(output["recommendation"]["first_issue"]["number"], 13);
    assert_eq!(output["comparison_to_runner_up"]["runner_up"]["number"], 1);
    assert_eq!(
        output["comparison_to_runner_up"]["reason_code"],
        "declared_priority_tiebreak"
    );
    assert_eq!(
        output["alternatives"]
            .as_array()
            .expect("bounded alternatives")
            .iter()
            .map(|candidate| candidate["first_issue"]["number"]
                .as_u64()
                .expect("Issue number"))
            .collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
    mocks.assert();
}

#[test]
fn no_candidate_succeeds_with_categories_and_deterministic_offline_json() {
    let mut github = Server::new();
    let api_url = github.url();
    let state = TempDir::new().expect("temporary state directory");
    let issues = vec![
        issue(1, "open", &[], &["alice"]),
        issue(2, "open", &[], &[]),
        issue(3, "open", &[], &[]),
        issue(4, "open", &[], &[]),
    ];
    let dependencies = vec![
        (1, vec![]),
        (2, vec![internal_blocker(3, "open")]),
        (3, vec![internal_blocker(2, "open")]),
        (4, vec![external_blocker("private/unknown", 99, "unknown")]),
    ];
    let mocks = mock_repository(&mut github, "acme/none", issues, dependencies);
    let online = next_command(&state, &api_url, "acme/none", true)
        .output()
        .expect("seed Local replica");
    assert_success(&online);
    mocks.assert();
    drop(github);

    let first = next_command(&state, &api_url, "acme/none", false)
        .output()
        .expect("first offline next");
    let second = next_command(&state, &api_url, "acme/none", false)
        .output()
        .expect("second offline next");
    assert_success(&first);
    assert_success(&second);
    assert_eq!(first.stdout, second.stdout);
    let output: Value = serde_json::from_slice(&first.stdout).expect("next JSON");
    assert_eq!(output["source"], "local_fallback");
    assert_eq!(output["recommendation"], Value::Null);
    assert_eq!(output["alternatives"], json!([]));
    assert_eq!(output["summary"]["blocked_count"], 3);
    assert_eq!(output["summary"]["assigned_ready_count"], 1);
    assert_eq!(output["summary"]["cyclic_issue_count"], 2);
    assert_eq!(output["summary"]["unknown_blocker_count"], 1);
    assert_eq!(output["warnings"][0]["code"], "offline_fallback");

    let assigned = next_command(&state, &api_url, "acme/none", false)
        .arg("--assignee")
        .arg("alice")
        .output()
        .expect("offline assignee next");
    assert_success(&assigned);
    let assigned: Value = serde_json::from_slice(&assigned.stdout).expect("assignee next JSON");
    assert_eq!(
        assigned["execution_scope"],
        json!({"mode": "assignee", "assignee": "alice"})
    );
    assert_eq!(assigned["recommendation"]["first_issue"]["number"], 1);
    assert_ne!(assigned["input_hash"], output["input_hash"]);
}

#[test]
fn empty_graph_omits_pagerank_globally_and_out_of_range_horizon_is_rejected() {
    let mut github = Server::new();
    let state = TempDir::new().expect("temporary state directory");
    let mocks = mock_repository(&mut github, "acme/empty", vec![], vec![]);
    let output = next_command(&state, &github.url(), "acme/empty", true)
        .output()
        .expect("run empty next");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("next JSON");
    assert_eq!(output["metrics"]["pagerank"]["state"], "omitted");
    assert_eq!(output["recommendation"], Value::Null);
    assert_eq!(output["alternatives"], json!([]));
    mocks.assert();

    let unsupported = Command::new(env!("CARGO_BIN_EXE_grit"))
        .args(["next", "--repo", "acme/empty", "--horizon", "4", "--json"])
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "")
        .output()
        .expect("unsupported horizon");
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("between 1 and 3"));
}

fn next_command(state: &TempDir, api_url: &str, repository: &str, online: bool) -> Command {
    next_command_with_horizon(state, api_url, repository, online, Some(1))
}

fn next_default_command(state: &TempDir, api_url: &str, repository: &str, online: bool) -> Command {
    next_command_with_horizon(state, api_url, repository, online, None)
}

fn next_default_human_command(state: &TempDir, api_url: &str, repository: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["next", "--repo", repository]);
    command
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env_remove("GH_TOKEN")
        .env("PATH", "");
    command
}

fn next_command_with_horizon(
    state: &TempDir,
    api_url: &str,
    repository: &str,
    online: bool,
    horizon: Option<u8>,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["next", "--repo", repository, "--json"]);
    if let Some(horizon) = horizon {
        command.args(["--horizon", &horizon.to_string()]);
    }
    command
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env("PATH", "");
    if online {
        command.env("GH_TOKEN", "automation-token");
    } else {
        command.env_remove("GH_TOKEN");
    }
    command
}

fn rollout_numbers(candidate: &Value) -> Vec<u64> {
    candidate["rollout"]["steps"]
        .as_array()
        .expect("rollout steps")
        .iter()
        .map(|step| step["issue"]["number"].as_u64().expect("step Issue number"))
        .collect()
}

fn unlock_numbers(candidate: &Value) -> Vec<u64> {
    candidate["outcome"]["unlocks"]
        .as_array()
        .expect("unlocks")
        .iter()
        .map(|unlock| {
            unlock["issue"]["number"]
                .as_u64()
                .expect("unlocked Issue number")
        })
        .collect()
}

fn ready_command(state: &TempDir, api_url: &str, repository: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grit"));
    command.args(["ready", "--repo", repository, "--json"]);
    command
        .env("GRIT_GITHUB_API_URL", api_url)
        .env("GRIT_STATE_DIR", state.path())
        .env_remove("GH_TOKEN")
        .env("PATH", "");
    command
}

struct RepositoryMocks {
    labels: Mock,
    issues: Mock,
    comments: Mock,
    dependencies: Vec<Mock>,
}

impl RepositoryMocks {
    fn assert(self) {
        self.labels.assert();
        self.issues.assert();
        self.comments.assert();
        for dependency in self.dependencies {
            dependency.assert();
        }
    }
}

fn mock_repository(
    github: &mut Server,
    repository: &str,
    issues: Vec<Value>,
    dependencies: Vec<(u64, Vec<Value>)>,
) -> RepositoryMocks {
    let labels_path = format!("/repos/{repository}/labels");
    let labels = github
        .mock("GET", labels_path.as_str())
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(canonical_labels().to_string())
        .create();
    let issues_path = format!("/repos/{repository}/issues");
    let issue_response = Value::Array(issues);
    let issues = github
        .mock("GET", issues_path.as_str())
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(issue_response.to_string())
        .create();
    let comments_path = format!("/repos/{repository}/issues/comments");
    let comments = github
        .mock("GET", comments_path.as_str())
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let dependencies = dependencies
        .into_iter()
        .map(|(number, blockers)| {
            let blockers = blockers
                .into_iter()
                .map(|mut blocker| {
                    if blocker["repository_url"] == "https://api.github.com/repos/acme/placeholder"
                    {
                        blocker["repository_url"] =
                            Value::String(format!("https://api.github.com/repos/{repository}"));
                    }
                    blocker
                })
                .collect();
            let path = format!("/repos/{repository}/issues/{number}/dependencies/blocked_by");
            github
                .mock("GET", path.as_str())
                .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(Value::Array(blockers).to_string())
                .create()
        })
        .collect();
    RepositoryMocks {
        labels,
        issues,
        comments,
        dependencies,
    }
}

fn issue(number: u64, state: &str, priority_labels: &[&str], assignees: &[&str]) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "number": number,
        "title": format!("Issue {number}"),
        "body": "",
        "state": state,
        "state_reason": if state == "closed" { Some("completed") } else { None },
        "html_url": format!("https://github.com/acme/repo/issues/{number}"),
        "user": null,
        "assignees": assignees
            .iter()
            .enumerate()
            .map(|(index, login)| actor(number * 1000 + index as u64, login))
            .collect::<Vec<_>>(),
        "labels": priority_labels
            .iter()
            .enumerate()
            .map(|(index, name)| label(number * 10 + index as u64, name))
            .collect::<Vec<_>>(),
        "created_at": "2026-08-01T00:00:00Z",
        "updated_at": "2026-08-01T00:00:00Z",
        "closed_at": if state == "closed" { Some("2026-08-02T00:00:00Z") } else { None }
    })
}

fn internal_blocker(number: u64, state: &str) -> Value {
    blocker("acme/placeholder", number, state)
}

fn external_blocker(repository: &str, number: u64, state: &str) -> Value {
    blocker(repository, number, state)
}

fn blocker(repository: &str, number: u64, state: &str) -> Value {
    json!({
        "id": number * 100,
        "node_id": format!("I_{number}"),
        "repository_url": format!("https://api.github.com/repos/{repository}"),
        "number": number,
        "state": state
    })
}

fn canonical_labels() -> Value {
    json!([
        label(1, "priority:p0"),
        label(2, "priority:p1"),
        label(3, "priority:p2"),
        label(4, "priority:p3"),
        label(5, "priority:p4")
    ])
}

fn label(id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("L_{id}"),
        "name": name,
        "color": "123456",
        "description": null
    })
}

fn actor(id: u64, login: &str) -> Value {
    json!({"id": id, "node_id": format!("U_{id}"), "login": login})
}

fn reason_codes(output: &Value) -> Vec<&str> {
    output["recommendation"]["reasons"]
        .as_array()
        .expect("recommendation reasons")
        .iter()
        .map(|reason| reason["code"].as_str().expect("reason code"))
        .collect()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
