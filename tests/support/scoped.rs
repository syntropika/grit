use mockito::{Matcher, Mock, Server, ServerGuard};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    process::Command,
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

pub struct Fixture {
    pub server: ServerGuard,
    pub state: TempDir,
    pub remote: Arc<Mutex<Remote>>,
    _mocks: Vec<Mock>,
}

#[derive(Default)]
pub struct Remote {
    pub issues: BTreeMap<u64, Value>,
    pub children: BTreeMap<u64, Vec<u64>>,
    pub external_children: BTreeMap<u64, Vec<Value>>,
    pub blockers: BTreeMap<u64, Vec<u64>>,
    pub writes: Vec<String>,
    pub invalid_relationship_inventory: bool,
    pub lose_create_response: bool,
}

pub fn issue(number: u64, labels: &[&str]) -> Value {
    json!({"id": number * 100, "node_id": format!("I_{number}"), "number": number,
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "repository_url": "https://api.github.com/repos/acme/widgets",
        "title": format!("Work {number}"), "body": format!("Specification for {number}"), "state": "open", "state_reason": null,
        "user": {"id": 1, "node_id": "U_1", "login": "alice"}, "assignees": [],
        "labels": labels.iter().map(|name| json!({"id": 1, "node_id": "L_1", "name": name, "color": "ffffff"})).collect::<Vec<_>>(),
        "created_at": "2026-09-22T00:00:00Z", "updated_at": "2026-09-22T00:00:00Z", "closed_at": null})
}

impl Fixture {
    pub fn new() -> Self {
        let mut server = Server::new();
        let state = TempDir::new().unwrap();
        let mut data = Remote::default();
        for number in 1..=8 {
            let labels: &[&str] = match number {
                1 => &["type:planning"],
                4 => &["priority:p0"],
                _ => &["area:foundation", "ready-for-agent"],
            };
            data.issues.insert(number, issue(number, labels));
        }
        data.issues.get_mut(&8).unwrap()["state"] = json!("closed");
        data.children.insert(1, vec![2, 3, 6]);
        data.children.insert(3, vec![7]);
        data.blockers.insert(2, vec![8]);
        data.blockers.insert(3, vec![2]);
        data.blockers.insert(6, vec![4]);
        data.blockers.insert(7, vec![3]);
        let remote = Arc::new(Mutex::new(data));
        let mut mocks = Vec::new();
        for path in ["labels", "issues/events"] {
            mocks.push(
                server
                    .mock("GET", format!("/repos/acme/widgets/{path}").as_str())
                    .match_query(Matcher::Any)
                    .with_status(200)
                    .with_body("[]")
                    .create(),
            );
        }
        let inventory = remote.clone();
        mocks.push(
            server
                .mock("GET", "/repos/acme/widgets/issues")
                .match_query(Matcher::Any)
                .with_status(200)
                .with_body_from_request(move |_| {
                    json!(
                        inventory
                            .lock()
                            .unwrap()
                            .issues
                            .values()
                            .collect::<Vec<_>>()
                    )
                    .to_string()
                    .into_bytes()
                })
                .create(),
        );
        mocks.push(server.mock("GET", "/repos/acme/widgets/issues/comments").match_query(Matcher::Any).with_status(200)
            .with_body(json!([{"id": 10, "node_id": "C_10", "html_url": "https://github.com/acme/widgets/issues/2#issuecomment-10",
                "issue_url": "https://api.github.com/repos/acme/widgets/issues/2", "body": "Recorded discussion", "user": {"id":1,"node_id":"U_1","login":"alice"},
                "author_association":"OWNER", "created_at":"2026-09-22T00:00:00Z", "updated_at":"2026-09-22T00:00:00Z"}]).to_string()).create());
        for number in 1..=20 {
            let path = format!("/repos/acme/widgets/issues/{number}");
            let data = remote.clone();
            mocks.push(
                server
                    .mock("GET", path.as_str())
                    .with_status(200)
                    .with_body_from_request(move |_| {
                        data.lock().unwrap().issues[&number]
                            .to_string()
                            .into_bytes()
                    })
                    .create(),
            );
            let data = remote.clone();
            mocks.push(
                server
                    .mock("GET", format!("{path}/dependencies/blocked_by").as_str())
                    .match_query(Matcher::Any)
                    .with_status(200)
                    .with_body_from_request(move |_| {
                        let data = data.lock().unwrap();
                        json!(
                            data.blockers
                                .get(&number)
                                .into_iter()
                                .flatten()
                                .map(|n| &data.issues[n])
                                .collect::<Vec<_>>()
                        )
                        .to_string()
                        .into_bytes()
                    })
                    .create(),
            );
            let data = remote.clone();
            mocks.push(
                server
                    .mock("GET", format!("{path}/sub_issues").as_str())
                    .match_query(Matcher::Any)
                    .with_status(200)
                    .with_body_from_request(move |_| {
                        let data = data.lock().unwrap();
                        if data.invalid_relationship_inventory {
                            return b"{}".to_vec();
                        }
                        let mut children = data
                            .children
                            .get(&number)
                            .into_iter()
                            .flatten()
                            .map(|n| &data.issues[n])
                            .collect::<Vec<_>>();
                        children.extend(data.external_children.get(&number).into_iter().flatten());
                        json!(children).to_string().into_bytes()
                    })
                    .create(),
            );
            let parent = if [2, 3, 6].contains(&number) {
                Some(1)
            } else if number == 7 {
                Some(3)
            } else {
                None
            };
            mocks.push(
                server
                    .mock("GET", format!("{path}/parent").as_str())
                    .with_status(if parent.is_some() { 200 } else { 404 })
                    .with_body(
                        parent
                            .map(|n| issue(n, &[]).to_string())
                            .unwrap_or_else(|| "{}".to_owned()),
                    )
                    .create(),
            );
            let data = remote.clone();
            mocks.push(server.mock("POST", format!("{path}/labels").as_str()).with_status(200).with_body_from_request(move |request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                let mut data = data.lock().unwrap();
                data.writes.push(format!("label:{number}"));
                for name in body["labels"].as_array().unwrap() {
                    let labels = data.issues.get_mut(&number).unwrap()["labels"].as_array_mut().unwrap();
                    if !labels.iter().any(|label| label["name"] == *name) { labels.push(json!({"id": 1, "node_id": "L_1", "name": name, "color": "ffffff"})); }
                }
                data.issues[&number]["labels"].to_string().into_bytes()
            }).create());
            for priority in ["p0", "p1", "p2", "p3", "p4"] {
                let data = remote.clone();
                mocks.push(
                    server
                        .mock(
                            "DELETE",
                            Matcher::Regex(format!(
                                "^{path}/labels/priority(%3A|%3a|:){priority}$"
                            )),
                        )
                        .with_status(200)
                        .with_body_from_request(move |_| {
                            let mut data = data.lock().unwrap();
                            data.writes.push(format!("unlabel:{number}"));
                            data.issues.get_mut(&number).unwrap()["labels"]
                                .as_array_mut()
                                .unwrap()
                                .retain(|label| label["name"] != format!("priority:{priority}"));
                            data.issues[&number]["labels"].to_string().into_bytes()
                        })
                        .create(),
                );
            }
        }
        let data = remote.clone();
        mocks.push(
            server
                .mock("POST", "/repos/acme/widgets/issues")
                .with_status(201)
                .with_body_from_request(move |request| {
                    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                    let mut data = data.lock().unwrap();
                    let number = data.issues.keys().max().copied().unwrap_or(0) + 1;
                    let mut created = issue(number, &[]);
                    created["title"] = body["title"].clone();
                    created["body"] = body["body"].clone();
                    data.writes.push(format!("create:{number}"));
                    data.issues.insert(number, created.clone());
                    if data.lose_create_response {
                        b"{}".to_vec()
                    } else {
                        created.to_string().into_bytes()
                    }
                })
                .create(),
        );
        Self {
            server,
            state,
            remote,
            _mocks: mocks,
        }
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hyfa"));
        command
            .env("GH_TOKEN", "fixture-token")
            .env("HYFA_NO_KEYRING", "1")
            .env("HYFA_GITHUB_API_URL", self.server.url())
            .env("HYFA_STATE_DIR", self.state.path())
            .env("PATH", "");
        command
    }

    pub fn run(&self, args: &[&str], offline: bool) -> Value {
        let mut command = self.command();
        if offline {
            command.env_remove("GH_TOKEN");
        }
        let output = command.args(args).arg("--json").output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub fn snapshot(&self, name: &str) -> Vec<u8> {
        fs::read(
            self.state
                .path()
                .join("repositories/acme/widgets")
                .join(name),
        )
        .unwrap_or_default()
    }
}
