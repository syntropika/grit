use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

use mockito::{Matcher, Server};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn offline_comment_survives_restart_and_never_exposes_its_operation_marker() {
    let state = TempDir::new().expect("state directory");
    let mut github = Server::new();
    seed_replica(&mut github, &state, vec![issue(1)]);
    let unavailable = Server::new();

    let queued = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args([
            "comment",
            "acme/widgets#1",
            "--body",
            "Visible offline note\n\n<!-- grit-operation:6ba7b810-9dad-11d1-80b4-00c04fd430c8 -->",
            "--json",
        ])
        .output()
        .expect("queue comment");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("comment JSON");
    assert_eq!(queued["schema_version"], "grit.comment-create/v1");
    assert_eq!(queued["pending"], true);
    assert_eq!(queued["body"], "Visible offline note");

    let outbox = load_outbox(&state);
    let operation = &outbox["operations"][0];
    let marker = operation["marker"].as_str().expect("persisted marker");
    assert_eq!(operation["body"], "Visible offline note");
    assert!(!queued.to_string().contains(marker));
    assert!(
        !queued
            .to_string()
            .contains("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
    );

    let restarted = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args(["ready", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("restart with Pending comment");
    assert_success(&restarted);
    let restarted: Value = serde_json::from_slice(&restarted.stdout).expect("ready JSON");
    assert_eq!(restarted["pending"], true);
    assert_eq!(restarted["issues"][0]["pending"], true);
    assert!(!restarted.to_string().contains(marker));
    assert_eq!(
        load_outbox(&state)["operations"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn draft_comment_waits_for_identity_then_replays_to_the_mapped_issue() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state, Vec::new());
    let draft = grit(&state, &seed.url())
        .args([
            "create",
            "--repo",
            "acme/widgets",
            "--title",
            "Draft parent",
            "--json",
        ])
        .output()
        .expect("queue Draft");
    assert_success(&draft);
    let draft: Value = serde_json::from_slice(&draft.stdout).expect("Draft JSON");
    let key = draft["draft"]["key"].as_str().expect("Draft key");
    let unavailable = Server::new();
    let queued = grit(&state, &unavailable.url())
        .env_remove("GH_TOKEN")
        .args([
            "comment",
            key,
            "--body",
            "Comment authored offline",
            "--json",
        ])
        .output()
        .expect("queue Draft comment");
    assert_success(&queued);
    let queued: Value = serde_json::from_slice(&queued.stdout).expect("comment JSON");
    assert_eq!(
        queued["operation"]["depends_on"].as_array().unwrap().len(),
        1
    );
    let outbox = load_outbox(&state);
    let issue_marker = outbox["operations"][0]["marker"]
        .as_str()
        .expect("Issue marker")
        .to_owned();
    let comment_marker = outbox["operations"][1]["marker"]
        .as_str()
        .expect("comment marker")
        .to_owned();

    let remote = Arc::new(Mutex::new(RemoteRepository::default()));
    let mut github = Server::new();
    let inventory = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 2, 2, 1);
    let issue_create = mock_issue_create(&mut github, Arc::clone(&remote), 1);
    let comment_create = mock_comment_create(&mut github, &state, Arc::clone(&remote), 201, 1, 1);
    let reconciled = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("reconcile Draft comment");
    assert_success(&reconciled);
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout).expect("reconcile JSON");
    assert_eq!(reconciled["summary"]["applied"], 2, "{reconciled}");
    assert_eq!(reconciled["summary"]["remaining"], 0);
    assert!(!reconciled.to_string().contains(&issue_marker));
    assert!(!reconciled.to_string().contains(&comment_marker));

    let replica: Value = serde_json::from_slice(
        &fs::read(state.path().join("repositories/acme/widgets/replica.json")).expect("replica"),
    )
    .expect("replica JSON");
    assert_eq!(
        replica["issues"][0]["comments"][0]["body"],
        "Comment authored offline"
    );
    assert!(!replica.to_string().contains(&issue_marker));
    assert!(!replica.to_string().contains(&comment_marker));
    let remote = remote.lock().expect("remote");
    assert_eq!(remote.comment_requests, 1);
    assert_eq!(remote.comments.len(), 1);
    drop(remote);
    inventory.assert();
    issue_create.assert();
    comment_create.assert();
}

#[test]
fn accepted_comment_with_dropped_response_is_recovered_without_duplicate_create() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state, vec![issue(1)]);
    let remote = Arc::new(Mutex::new(RemoteRepository {
        issues: vec![issue(1)],
        ..RemoteRepository::default()
    }));
    let (dropped_url, dropped_server) = dropped_comment_server(&state, Arc::clone(&remote));

    let first = grit(&state, &dropped_url)
        .args([
            "comment",
            "acme/widgets#1",
            "--body",
            "Recover me exactly once",
            "--json",
        ])
        .output()
        .expect("submit ambiguous comment");
    assert_success(&first);
    dropped_server.join().expect("dropped-response server");
    let first: Value = serde_json::from_slice(&first.stdout).expect("comment JSON");
    assert_eq!(first["pending"], true, "{first}");
    let marker = load_outbox(&state)["operations"][0]["marker"]
        .as_str()
        .expect("marker")
        .to_owned();
    assert!(!first.to_string().contains(&marker));

    let mut github = Server::new();
    let inventory = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 2, 3, 2);
    let no_duplicate = github
        .mock("POST", "/repos/acme/widgets/issues/1/comments")
        .expect(0)
        .create();

    let recovered = grit(&state, &github.url())
        .args(["reconcile", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("recover ambiguous comment");
    assert_success(&recovered);
    let recovered: Value = serde_json::from_slice(&recovered.stdout).expect("reconcile JSON");
    assert_eq!(recovered["summary"]["already_satisfied"], 1, "{recovered}");
    assert_eq!(recovered["summary"]["remaining"], 0);
    assert!(!recovered.to_string().contains(&marker));
    let remote = remote.lock().expect("remote");
    assert_eq!(remote.comment_requests, 1);
    assert_eq!(remote.comments.len(), 1);
    drop(remote);
    inventory.assert();
    no_duplicate.assert();
}

#[test]
fn zero_or_multiple_comment_marker_matches_stay_unresolved_without_blind_retry() {
    for accepted_copies in [0, 2] {
        let state = TempDir::new().expect("state directory");
        let mut seed = Server::new();
        seed_replica(&mut seed, &state, vec![issue(1)]);
        let remote = Arc::new(Mutex::new(RemoteRepository {
            issues: vec![issue(1)],
            ..RemoteRepository::default()
        }));
        let mut github = Server::new();
        let inventory = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 4, 5, 4);
        let create = mock_comment_create(
            &mut github,
            &state,
            Arc::clone(&remote),
            500,
            accepted_copies,
            1,
        );
        let first = grit(&state, &github.url())
            .args([
                "comment",
                "acme/widgets#1",
                "--body",
                "Ambiguous note",
                "--json",
            ])
            .output()
            .expect("submit ambiguous comment");
        assert_success(&first);

        let unresolved = grit(&state, &github.url())
            .args(["reconcile", "--repo", "acme/widgets", "--json"])
            .output()
            .expect("inspect comment marker matches");
        assert_success(&unresolved);
        let unresolved: Value = serde_json::from_slice(&unresolved.stdout).expect("reconcile JSON");
        assert_eq!(unresolved["summary"]["remaining"], 1, "{unresolved}");
        let error = unresolved["operations"][0]["error"]
            .as_str()
            .expect("unresolved error");
        if accepted_copies == 0 {
            assert!(error.contains("no GitHub comment"), "{error}");
        } else {
            assert!(error.contains("2 GitHub comments"), "{error}");
            assert_eq!(unresolved["summary"]["conflicting"], 1);
        }
        let remote = remote.lock().expect("remote");
        assert_eq!(remote.comment_requests, 1);
        assert_eq!(remote.comments.len(), accepted_copies);
        drop(remote);
        inventory.assert();
        create.assert();
    }
}

#[test]
fn online_comment_is_persisted_before_write_then_published_from_verified_readback() {
    let state = TempDir::new().expect("state directory");
    let mut seed = Server::new();
    seed_replica(&mut seed, &state, vec![issue(1)]);
    let remote = Arc::new(Mutex::new(RemoteRepository {
        issues: vec![issue(1)],
        ..RemoteRepository::default()
    }));
    let mut github = Server::new();
    let inventory = mock_dynamic_inventory(&mut github, Arc::clone(&remote), 2, 2, 2);
    let create = mock_comment_create(&mut github, &state, Arc::clone(&remote), 201, 1, 1);
    let output = grit(&state, &github.url())
        .args([
            "comment",
            "acme/widgets#1",
            "--body",
            "Online note",
            "--json",
        ])
        .output()
        .expect("create online comment");
    assert_success(&output);
    let output: Value = serde_json::from_slice(&output.stdout).expect("comment JSON");
    assert_eq!(output["pending"], false, "{output}");
    assert_eq!(output["result"], "created");
    assert_eq!(output["remote"]["issue_number"], 1);
    assert_eq!(load_outbox(&state)["operations"], json!([]));
    let replica: Value = serde_json::from_slice(
        &fs::read(state.path().join("repositories/acme/widgets/replica.json")).expect("replica"),
    )
    .expect("replica JSON");
    assert_eq!(replica["issues"][0]["comments"][0]["body"], "Online note");
    inventory.assert();
    create.assert();
}

#[derive(Default)]
struct RemoteRepository {
    issues: Vec<Value>,
    comments: Vec<Value>,
    comment_requests: usize,
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
    comment_reads: usize,
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
    let comment_state = Arc::clone(&remote);
    let comments = github
        .mock("GET", "/repos/acme/widgets/issues/comments")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            json!(comment_state.lock().expect("remote").comments)
                .to_string()
                .into_bytes()
        })
        .expect(comment_reads)
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
        .expect(dependency_reads)
        .create();
    RepositoryMocks {
        mocks: vec![labels, issues, comments, dependencies],
    }
}

fn mock_issue_create(
    github: &mut Server,
    remote: Arc<Mutex<RemoteRepository>>,
    expected: usize,
) -> RepositoryMocks {
    let state = Arc::clone(&remote);
    let create = github
        .mock("POST", "/repos/acme/widgets/issues")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |request| {
            let payload: Value =
                serde_json::from_slice(request.body().expect("Issue body")).expect("Issue JSON");
            let mut remote = state.lock().expect("remote");
            let number = remote.issues.len() as u64 + 1;
            let mut created = issue(number);
            created["title"] = payload["title"].clone();
            created["body"] = payload["body"].clone();
            remote.issues.push(created.clone());
            created.to_string().into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks {
        mocks: vec![create],
    }
}

fn mock_comment_create(
    github: &mut Server,
    local_state: &TempDir,
    remote: Arc<Mutex<RemoteRepository>>,
    status: usize,
    accepted_copies: usize,
    expected: usize,
) -> RepositoryMocks {
    let state = Arc::clone(&remote);
    let outbox_path = local_state
        .path()
        .join("repositories/acme/widgets/outbox.json");
    let create = github
        .mock("POST", "/repos/acme/widgets/issues/1/comments")
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |request| {
            let payload: Value = serde_json::from_slice(request.body().expect("comment body"))
                .expect("comment JSON");
            let outbox: Value = serde_json::from_slice(
                &fs::read(&outbox_path).expect("outbox persisted before comment request"),
            )
            .expect("outbox JSON");
            let operation = outbox["operations"]
                .as_array()
                .expect("operations")
                .iter()
                .find(|operation| operation["kind"] == "comment_create")
                .expect("persisted comment operation");
            let marker = operation["marker"].as_str().expect("persisted marker");
            assert!(
                payload["body"]
                    .as_str()
                    .is_some_and(|body| body.contains(marker)),
                "request body must embed the marker persisted before the request"
            );
            assert!(!operation["body"].as_str().unwrap().contains(marker));
            let mut remote = state.lock().expect("remote");
            remote.comment_requests += 1;
            let first_id = remote.comments.len() as u64 + 1;
            for id in first_id..first_id + accepted_copies as u64 {
                remote.comments.push(comment(id, &payload["body"]));
            }
            remote
                .comments
                .last()
                .cloned()
                .unwrap_or_else(|| json!({"message": "not accepted"}))
                .to_string()
                .into_bytes()
        })
        .expect(expected)
        .create();
    RepositoryMocks {
        mocks: vec![create],
    }
}

fn comment(id: u64, body: &Value) -> Value {
    json!({
        "id": id * 1000,
        "node_id": format!("C_{id}"),
        "html_url": format!("https://github.com/acme/widgets/issues/1#issuecomment-{}", id * 1000),
        "body": body,
        "user": null,
        "author_association": "OWNER",
        "created_at": "2026-08-10T00:01:00Z",
        "updated_at": "2026-08-10T00:01:00Z",
        "issue_url": "https://api.github.com/repos/acme/widgets/issues/1"
    })
}

fn dropped_comment_server(
    local_state: &TempDir,
    remote: Arc<Mutex<RemoteRepository>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("dropped-response listener");
    let address = listener.local_addr().expect("listener address");
    let outbox_path = local_state
        .path()
        .join("repositories/acme/widgets/outbox.json");
    let server = thread::spawn(move || {
        let expected = [
            ("GET", "/repos/acme/widgets/labels"),
            ("GET", "/repos/acme/widgets/issues"),
            ("GET", "/repos/acme/widgets/issues/comments"),
            (
                "GET",
                "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            ),
            ("POST", "/repos/acme/widgets/issues/1/comments"),
            ("GET", "/repos/acme/widgets/labels"),
            ("GET", "/repos/acme/widgets/issues"),
            ("GET", "/repos/acme/widgets/issues/comments"),
            (
                "GET",
                "/repos/acme/widgets/issues/1/dependencies/blocked_by",
            ),
        ];
        for (expected_method, expected_path) in expected {
            let (mut stream, _) = listener.accept().expect("comment HTTP connection");
            let (method, path, body) = read_http_request(&mut stream);
            assert_eq!(method, expected_method);
            assert_eq!(path, expected_path);
            if method == "POST" {
                let payload: Value = serde_json::from_slice(&body).expect("comment request JSON");
                let outbox: Value = serde_json::from_slice(
                    &fs::read(&outbox_path).expect("durable outbox before comment request"),
                )
                .expect("outbox JSON");
                let operation = outbox["operations"]
                    .as_array()
                    .expect("operations")
                    .iter()
                    .find(|operation| operation["kind"] == "comment_create")
                    .expect("comment operation");
                let marker = operation["marker"].as_str().expect("persisted marker");
                assert!(payload["body"].as_str().unwrap().contains(marker));
                let mut remote = remote.lock().expect("remote");
                remote.comment_requests += 1;
                remote.comments.push(comment(1, &payload["body"]));
                continue;
            }
            let response = {
                let remote = remote.lock().expect("remote");
                match path.as_str() {
                    "/repos/acme/widgets/issues" => json!(remote.issues).to_string(),
                    "/repos/acme/widgets/issues/comments" => json!(remote.comments).to_string(),
                    _ => "[]".to_owned(),
                }
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .expect("HTTP response");
        }
    });
    (format!("http://{address}"), server)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> (String, String, Vec<u8>) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("HTTP request bytes");
        assert!(read > 0, "request ended before headers");
        request.extend_from_slice(&buffer[..read]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("HTTP request body");
        assert!(read > 0, "request ended before body");
        request.extend_from_slice(&buffer[..read]);
    }
    let request_line = headers.lines().next().expect("request line");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().expect("request method").to_owned();
    let target = parts.next().expect("request target");
    let path = target.split('?').next().expect("request path").to_owned();
    (
        method,
        path,
        request[header_end..header_end + content_length].to_vec(),
    )
}

fn seed_replica(github: &mut Server, state: &TempDir, issues: Vec<Value>) {
    let labels = github
        .mock("GET", "/repos/acme/widgets/labels")
        .match_query(Matcher::UrlEncoded("per_page".into(), "100".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    let issue_count = issues.len();
    let inventory = github
        .mock("GET", "/repos/acme/widgets/issues")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "all".into()),
            Matcher::UrlEncoded("sort".into(), "created".into()),
            Matcher::UrlEncoded("direction".into(), "asc".into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!(issues).to_string())
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
        .expect(issue_count)
        .create();
    let output = grit(state, &github.url())
        .args(["sync", "--repo", "acme/widgets", "--json"])
        .output()
        .expect("seed replica");
    assert_success(&output);
    labels.assert();
    inventory.assert();
    comments.assert();
    dependencies.assert();
}

fn load_outbox(state: &TempDir) -> Value {
    serde_json::from_slice(
        &fs::read(state.path().join("repositories/acme/widgets/outbox.json")).expect("outbox"),
    )
    .expect("outbox JSON")
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
        "created_at": "2026-08-10T00:00:00Z",
        "updated_at": "2026-08-10T00:00:00Z",
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
