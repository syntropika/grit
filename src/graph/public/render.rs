use schemars::schema_for;
use serde_json::Value;

use super::{ConfirmedPublicRepository, PublicGraph, PublicReadiness};
use crate::graph::{GraphError, serialization::pretty_json, text::escape_html};

pub(super) fn schema_json(repository: &ConfirmedPublicRepository) -> Result<Vec<u8>, GraphError> {
    let mut schema =
        serde_json::to_value(schema_for!(PublicGraph)).map_err(GraphError::EncodeArtifact)?;
    let url_pattern = format!(
        "^{}/issues/[1-9][0-9]*$",
        regex_escape(repository.web_url.as_str())
    );
    let url_schema = schema
        .pointer_mut("/$defs/public_node/properties/url")
        .ok_or(GraphError::InvalidField("public URL schema"))?;
    url_schema["pattern"] = Value::String(url_pattern);
    pretty_json(&schema)
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub(super) fn html(artifact: &PublicGraph) -> String {
    let include_labels = artifact.nodes.iter().any(|node| node.labels.is_some());
    let include_assignees = artifact.nodes.iter().any(|node| node.assignees.is_some());
    let mut rows = String::new();
    for node in &artifact.nodes {
        let labels = node
            .labels
            .as_ref()
            .map(|values| values.join(", "))
            .unwrap_or_default();
        let assignees = node
            .assignees
            .as_ref()
            .map(|values| values.join(", "))
            .unwrap_or_default();
        let layer = node
            .position
            .layer
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unresolved".to_owned());
        rows.push_str(&format!(
            "<tr><td><a href=\"{}\">{}</a></td><td>{}</td><td>{}</td><td>{layer}</td>",
            escape_html(&node.url),
            escape_html(&node.key.to_string()),
            escape_html(&node.title),
            readiness_name(node.readiness),
        ));
        if include_labels {
            rows.push_str(&format!("<td>{}</td>", escape_html(&labels)));
        }
        if include_assignees {
            rows.push_str(&format!("<td>{}</td>", escape_html(&assignees)));
        }
        rows.push_str("</tr>");
    }
    let mut optional_headers = String::new();
    if include_labels {
        optional_headers.push_str("<th scope=\"col\">Labels</th>");
    }
    if include_assignees {
        optional_headers.push_str("<th scope=\"col\">Assignees</th>");
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'\"><title>Public Issue graph for {repository}</title><link rel=\"alternate\" type=\"application/json\" href=\"./graph.json\"></head><body><main><h1>Public Issue graph for {repository}</h1><p>Snapshot <code>{synced_at}</code> · artifact <code>{artifact_hash}</code></p><table><caption>Allowlisted public Issue graph</caption><thead><tr><th scope=\"col\">Issue</th><th scope=\"col\">Title</th><th scope=\"col\">Readiness</th><th scope=\"col\">Layer</th>{optional_headers}</tr></thead><tbody>{rows}</tbody></table></main></body></html>\n",
        repository = escape_html(&artifact.repository),
        synced_at = escape_html(&artifact.synced_at),
        artifact_hash = escape_html(&artifact.artifact_hash),
    )
}

fn readiness_name(readiness: PublicReadiness) -> &'static str {
    match readiness {
        PublicReadiness::Ready => "ready",
        PublicReadiness::Blocked => "blocked",
        PublicReadiness::ExternalUnknown => "external_unknown",
    }
}
