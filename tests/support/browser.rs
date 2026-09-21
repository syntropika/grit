use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, connect, stream::MaybeTlsStream};

pub(crate) struct BrowserAudit {
    pub(crate) result: Value,
    pub(crate) requests: Vec<String>,
}

pub(crate) fn audit_local_page(
    browser_binary: &OsStr,
    profile: &Path,
    page: &Path,
) -> BrowserAudit {
    let mut browser = BrowserProcess::spawn(browser_binary, profile);
    let port = wait_for_debugging_port(profile, &mut browser.child);
    let targets: Value = reqwest::blocking::get(format!("http://127.0.0.1:{port}/json/list"))
        .expect("query Chrome DevTools targets")
        .json()
        .expect("Chrome DevTools target JSON");
    let websocket_url = targets
        .as_array()
        .expect("Chrome target array")
        .iter()
        .find(|target| target["type"] == "page")
        .and_then(|target| target["webSocketDebuggerUrl"].as_str())
        .expect("Chrome page WebSocket URL");
    let (socket, _) = connect(websocket_url).expect("connect to Chrome DevTools");
    let mut session = DevToolsSession::new(socket);
    session.command("Network.enable", json!({}));
    session.command("Page.enable", json!({}));
    let page_url = url::Url::from_file_path(page)
        .expect("local browser page URL")
        .to_string();
    session.command("Page.navigate", json!({"url": page_url}));
    session.wait_for_load();

    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        let evaluation = session.command(
            "Runtime.evaluate",
            json!({
                "expression": "document.querySelector('#result')?.textContent ?? 'pending'",
                "returnByValue": true
            }),
        );
        let value = evaluation
            .pointer("/result/value")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        if value != "pending" {
            break serde_json::from_str(value).expect("browser audit result JSON");
        }
        assert!(Instant::now() < deadline, "browser audit timed out");
        thread::sleep(Duration::from_millis(20));
    };
    let requests = session.requests;
    let _ = session.socket.send(Message::Text(
        json!({"id": session.next_id, "method": "Browser.close"})
            .to_string()
            .into(),
    ));
    BrowserAudit { result, requests }
}

struct BrowserProcess {
    child: Child,
}

impl BrowserProcess {
    fn spawn(browser_binary: &OsStr, profile: &Path) -> Self {
        let child = Command::new(browser_binary)
            .arg("--headless=new")
            .arg("--no-sandbox")
            .arg("--disable-gpu")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-default-apps")
            .arg("--disable-sync")
            .arg("--metrics-recording-only")
            .arg("--no-first-run")
            .arg("--allow-file-access-from-files")
            .arg("--host-resolver-rules=MAP * ~NOTFOUND")
            .arg("--remote-allow-origins=*")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch Google Chrome for DevTools audit");
        Self { child }
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_debugging_port(profile: &Path, child: &mut Child) -> u16 {
    let port_file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = fs::read_to_string(&port_file)
            && let Some(port) = contents.lines().next()
        {
            return port.parse().expect("Chrome DevTools port");
        }
        assert!(
            child.try_wait().expect("Chrome process state").is_none(),
            "Chrome exited before exposing DevTools"
        );
        assert!(Instant::now() < deadline, "Chrome DevTools timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

struct DevToolsSession {
    socket: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    next_id: u64,
    loaded: bool,
    requests: Vec<String>,
}

impl DevToolsSession {
    fn new(socket: WebSocket<MaybeTlsStream<std::net::TcpStream>>) -> Self {
        Self {
            socket,
            next_id: 1,
            loaded: false,
            requests: Vec::new(),
        }
    }

    fn command(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.socket
            .send(Message::Text(
                json!({"id": id, "method": method, "params": params})
                    .to_string()
                    .into(),
            ))
            .expect("send Chrome DevTools command");
        loop {
            let message = self.read_message();
            if message["id"] == id {
                assert!(
                    message.get("error").is_none(),
                    "Chrome DevTools command failed: {message}"
                );
                return message["result"].clone();
            }
        }
    }

    fn wait_for_load(&mut self) {
        while !self.loaded {
            self.read_message();
        }
    }

    fn read_message(&mut self) -> Value {
        loop {
            let message = self.socket.read().expect("Chrome DevTools message");
            let Message::Text(text) = message else {
                continue;
            };
            let document: Value = serde_json::from_str(&text).expect("Chrome DevTools JSON");
            match document["method"].as_str() {
                Some("Page.loadEventFired") => self.loaded = true,
                Some("Network.requestWillBeSent") | Some("Network.webSocketCreated") => {
                    if let Some(url) = document
                        .pointer("/params/request/url")
                        .and_then(Value::as_str)
                        .or_else(|| document.pointer("/params/url").and_then(Value::as_str))
                    {
                        self.requests.push(url.to_owned());
                    }
                }
                _ => {}
            }
            return document;
        }
    }
}
