use schemars::schema_for;
use serde::Serialize;

use super::{GraphError, artifact::GraphArtifact};

pub(super) fn graph_json(artifact: &GraphArtifact) -> Result<Vec<u8>, GraphError> {
    pretty_json(artifact)
}

pub(super) fn schema_json() -> Result<Vec<u8>, GraphError> {
    pretty_json(&schema_for!(GraphArtifact))
}

pub(super) fn html(artifact: &GraphArtifact) -> String {
    let mut rows = String::new();
    for node in &artifact.nodes {
        let key = escape_html(&node.key.to_string());
        let title = escape_html(node.title.as_deref().unwrap_or(&key));
        let issue = node
            .url
            .as_deref()
            .map(|url| format!("<a href=\"{}\">{key}</a>", escape_html(url)))
            .unwrap_or(key);
        let layer = node
            .position
            .layer
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unresolved".to_owned());
        rows.push_str(&format!(
            "<tr><td>{issue}</td><td>{title}</td><td>{}</td><td>{}</td><td>{layer}</td><td>{}</td></tr>",
            escape_html(&node.state),
            node.readiness.as_str(),
            escape_html(&node.assignees.join(", "))
        ));
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Issue graph for {repository}</title><link rel=\"alternate\" type=\"application/json\" href=\"./graph.json\"><style>body{{font-family:system-ui,sans-serif;margin:2rem;color:#171717}}table{{border-collapse:collapse;width:100%}}caption{{font-size:1.25rem;font-weight:700;text-align:left;margin-bottom:1rem}}th,td{{border:1px solid #ccc;padding:.5rem;text-align:left}}th{{background:#f4f4f4}}code{{font-family:ui-monospace,monospace}}</style></head><body><main><h1>Grit Issue graph</h1><p>Snapshot <code>{synced_at}</code> · artifact <code>{artifact_hash}</code></p><table><caption>Issue graph for {repository}</caption><thead><tr><th scope=\"col\">Issue</th><th scope=\"col\">Title</th><th scope=\"col\">State</th><th scope=\"col\">Readiness</th><th scope=\"col\">Layer</th><th scope=\"col\">Assignees</th></tr></thead><tbody>{rows}</tbody></table></main></body></html>\n",
        repository = escape_html(&artifact.repository),
        synced_at = escape_html(&artifact.synced_at),
        artifact_hash = escape_html(&artifact.artifact_hash),
    )
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, GraphError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(GraphError::EncodeArtifact)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
