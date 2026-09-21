use std::{
    collections::BTreeSet,
    fs,
    process::Command,
    time::{Duration, Instant},
};

use serde_json::json;
use tempfile::TempDir;

use super::{artifact, layout, presentation, render};
use crate::model::{BlockerIdentity, BlockerScope, Dependency, Issue, IssueIdentity, LocalReplica};

#[test]
#[ignore = "manual release-mode browser benchmark; run as documented in docs/benchmarks/graph-browser.md"]
fn dense_graph_browser_benchmark() {
    let browser = std::env::var_os("GRIT_BROWSER").unwrap_or_else(|| "google-chrome".into());
    for (node_count, edge_count) in [
        (100_usize, 400_usize),
        (1_000, 4_000),
        (5_000, 20_000),
        (10_000, 40_000),
    ] {
        let replica = synthetic_replica(node_count, edge_count);
        let artifact_started = Instant::now();
        let artifact = artifact::build(
            &replica,
            crate::operational::ExecutionScope::Available,
            crate::ranking::DEFAULT_HORIZON,
        )
        .expect("synthetic graph artifact");
        let artifact_build = artifact_started.elapsed();

        let mut positioned_nodes = artifact.nodes.clone();
        let layout_started = Instant::now();
        layout::assign_artifact_dependency_layers(&mut positioned_nodes, &artifact.edges)
            .expect("synthetic graph layout");
        let layout = layout_started.elapsed();

        let serialization_started = Instant::now();
        let graph_json = render::graph_json(&artifact).expect("synthetic artifact JSON");
        let serialization = serialization_started.elapsed();
        let schema_json = render::schema_json().expect("synthetic artifact schema");
        let presentation_started = Instant::now();
        let presentation = presentation::build(&artifact);
        let presentation_build = presentation_started.elapsed();
        let html_started = Instant::now();
        let html = render::html(&artifact, &presentation).expect("synthetic graph HTML");
        let html_render = html_started.elapsed();

        let site = TempDir::new().expect("benchmark site");
        fs::write(site.path().join("index.html"), &html).expect("benchmark HTML");
        fs::write(site.path().join("app.css"), render::stylesheet()).expect("benchmark CSS");
        fs::write(
            site.path().join("network-view.js"),
            render::network_view_javascript(),
        )
        .expect("benchmark network-view JavaScript");
        fs::write(
            site.path().join("graph-query.js"),
            render::graph_query_javascript(),
        )
        .expect("benchmark graph-query JavaScript");
        fs::write(site.path().join("app.js"), render::javascript()).expect("benchmark JavaScript");
        fs::write(
            site.path().join("graph-camera.js"),
            render::graph_camera_javascript(),
        )
        .expect("benchmark camera JavaScript");
        fs::write(site.path().join("graph.json"), &graph_json).expect("benchmark graph JSON");
        fs::write(site.path().join("graph.schema.json"), &schema_json)
            .expect("benchmark graph schema");
        let profile = TempDir::new().expect("benchmark browser profile");
        let output = Command::new(&browser)
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
            .arg("--virtual-time-budget=10000")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("--dump-dom")
            .arg(format!("file://{}/index.html", site.path().display()))
            .output()
            .expect("launch benchmark browser");
        assert!(
            output.status.success(),
            "browser benchmark failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let dom = String::from_utf8(output.stdout).expect("benchmark browser DOM");
        let load_ms = attribute(&dom, "data-grit-load-ms");
        let browser_render_ms = attribute(&dom, "data-grit-render-ms");
        let time_to_interactive_ms = attribute(&dom, "data-grit-time-to-interactive-ms");
        let mode = attribute_text(&dom, "data-grit-network-mode");
        let rendered_nodes = attribute(&dom, "data-grit-rendered-nodes") as usize;
        let rendered_edges = attribute(&dom, "data-grit-rendered-edges") as usize;
        let expected_mode = if node_count > presentation::FULL_NETWORK_MAX_NODES
            || edge_count > presentation::FULL_NETWORK_MAX_EDGES
        {
            "constrained"
        } else {
            "full"
        };
        assert_eq!(mode, expected_mode);
        if mode == "constrained" {
            assert!(rendered_nodes <= presentation::INITIAL_NETWORK_MAX_NODES);
        } else {
            assert_eq!(rendered_nodes, node_count);
            assert_eq!(rendered_edges, edge_count);
        }
        let site_size = graph_json.len()
            + schema_json.len()
            + html.len()
            + render::stylesheet().len()
            + render::network_view_javascript().len()
            + render::graph_query_javascript().len()
            + render::graph_camera_javascript().len()
            + render::javascript().len();

        println!(
            "{}",
            json!({
                "nodes": node_count,
                "edges": edge_count,
                "artifact_bytes": graph_json.len(),
                "site_bytes": site_size,
                "artifact_build_ms": milliseconds(artifact_build),
                "layout_ms": milliseconds(layout),
                "presentation_build_ms": milliseconds(presentation_build),
                "serialization_ms": milliseconds(serialization),
                "html_render_ms": milliseconds(html_render),
                "browser_load_ms": load_ms,
                "browser_render_ms": browser_render_ms,
                "time_to_interactive_ms": time_to_interactive_ms,
                "network_mode": mode,
                "rendered_nodes": rendered_nodes,
                "rendered_edges": rendered_edges,
            })
        );
    }
}

fn synthetic_replica(node_count: usize, edge_count: usize) -> LocalReplica {
    let issues = (1..=node_count as u64).map(issue).collect::<Vec<_>>();
    let width = (node_count as f64).sqrt().ceil() as usize;
    let mut pairs = BTreeSet::new();
    for lane in 1..node_count {
        let distance = lane * width;
        if distance >= node_count {
            break;
        }
        for blocked in (distance + 1)..=node_count {
            pairs.insert((blocked as u64, (blocked - distance) as u64));
            if pairs.len() == edge_count {
                break;
            }
        }
        if pairs.len() == edge_count {
            break;
        }
    }
    assert_eq!(pairs.len(), edge_count, "synthetic edge inventory");
    let dependencies = pairs
        .into_iter()
        .map(|(blocked, blocker)| dependency(blocked, blocker))
        .collect();
    LocalReplica::build_with_sync(
        "benchmark/issues".to_owned(),
        "2026-08-07T00:00:00Z".to_owned(),
        Default::default(),
        Vec::new(),
        issues,
        dependencies,
    )
    .expect("synthetic Local replica")
}

fn issue(number: u64) -> Issue {
    Issue {
        identity: crate::model::IssueIdentityState::GitHub,
        id: number,
        node_id: format!("I_{number}"),
        number,
        url: format!("https://github.com/benchmark/issues/issues/{number}"),
        title: format!("Synthetic Issue {number}"),
        body: String::new(),
        state: "open".to_owned(),
        state_reason: None,
        author: None,
        assignees: Vec::new(),
        labels: Vec::new(),
        comments: Vec::new(),
        created_at: "2026-08-01T00:00:00Z".to_owned(),
        updated_at: "2026-08-01T00:00:00Z".to_owned(),
        closed_at: None,
    }
}

fn dependency(blocked: u64, blocker: u64) -> Dependency {
    Dependency {
        blocked: IssueIdentity {
            repository: "benchmark/issues".to_owned(),
            number: blocked,
            id: blocked,
            node_id: format!("I_{blocked}"),
        },
        blocker: BlockerIdentity {
            repository: "benchmark/issues".to_owned(),
            number: blocker,
            state: "open".to_owned(),
            scope: BlockerScope::Internal,
            id: Some(blocker),
            node_id: Some(format!("I_{blocker}")),
        },
    }
}

fn attribute(dom: &str, name: &str) -> f64 {
    attribute_text(dom, name)
        .parse()
        .unwrap_or_else(|_| panic!("numeric browser metric {name}"))
}

fn attribute_text<'a>(dom: &'a str, name: &str) -> &'a str {
    let marker = format!("{name}=\"");
    let start = dom.find(&marker).expect("browser metric") + marker.len();
    let end = start + dom[start..].find('"').expect("browser metric terminator");
    &dom[start..end]
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
