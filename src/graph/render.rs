use std::collections::BTreeMap;

use schemars::schema_for;

use super::{
    GraphError,
    artifact::{ArtifactNode, GraphArtifact},
    presentation::GraphPresentation,
    serialization::pretty_json,
    text::escape_html,
};

pub(super) fn graph_json(artifact: &GraphArtifact) -> Result<Vec<u8>, GraphError> {
    pretty_json(artifact)
}

pub(super) fn schema_json() -> Result<Vec<u8>, GraphError> {
    pretty_json(&schema_for!(GraphArtifact))
}

pub(super) fn html(
    artifact: &GraphArtifact,
    presentation: &GraphPresentation,
) -> Result<String, GraphError> {
    let show_projects = artifact.nodes.iter().any(|node| node.projects().is_some());
    let mut blockers = BTreeMap::<String, Vec<String>>::new();
    let mut dependents = BTreeMap::<String, Vec<String>>::new();
    for edge in &artifact.edges {
        let blocked = edge.blocked.to_string();
        let blocker = edge.blocker.to_string();
        blockers
            .entry(blocked.clone())
            .or_default()
            .push(blocker.clone());
        dependents.entry(blocker).or_default().push(blocked);
    }

    let mut rows = String::new();
    for node in &artifact.nodes {
        let key = node.key().to_string();
        let escaped_key = escape_html(&key);
        let (title, url, priority, unlock_count, pagerank, assignees, labels) = match node {
            ArtifactNode::Issue {
                title,
                url,
                priority,
                unlock_count,
                pagerank_bucket,
                assignees,
                labels,
                ..
            } => (
                title.as_str(),
                Some(url.as_str()),
                priority.display_name(),
                unlock_count.map(|value| value.to_string()),
                pagerank_bucket.map(|value| value.to_string()),
                assignees.as_slice(),
                labels.as_slice(),
            ),
            ArtifactNode::ExternalBlocker { .. } => {
                (key.as_str(), None, "—", None, None, &[][..], &[][..])
            }
        };
        let title = escape_html(title);
        let layer = node
            .position()
            .layer
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unresolved / SCC".to_owned());
        let github_link = url
            .map(|url| {
                format!(
                    " <a class=\"canonical-link\" href=\"{}\" aria-label=\"Open {} on GitHub\">GitHub</a>",
                    escape_html(url), escaped_key
                )
            })
            .unwrap_or_default();
        let project_cell = if show_projects {
            let projects = node
                .projects()
                .map(|values| values.join(", "))
                .unwrap_or_else(|| "—".to_owned());
            format!("<td>{}</td>", escape_html(&projects))
        } else {
            String::new()
        };
        rows.push_str(&format!(
            "<tr data-node-key=\"{escaped_key}\"><td><button type=\"button\" class=\"table-node\" data-node-key=\"{escaped_key}\" aria-pressed=\"false\">{escaped_key}</button>{github_link}</td><td>{title}</td><td>{state}</td><td>{readiness}</td><td>{priority}</td><td>{unlock_count}</td><td>{pagerank}</td><td>{layer}</td><td>{assignees}</td><td>{labels}</td>{project_cell}<td class=\"blockers-cell\">{blockers}</td><td class=\"dependents-cell\">{dependents}</td><td class=\"relationship-cell\">—</td></tr>",
            state = escape_html(node.lifecycle()),
            readiness = node.status(),
            priority = priority,
            unlock_count = unlock_count.unwrap_or_else(|| "—".to_owned()),
            pagerank = pagerank.unwrap_or_else(|| "—".to_owned()),
            assignees = escape_html(&assignees.join(", ")),
            labels = escape_html(&labels.join(", ")),
            blockers = escape_html(&joined_relations(&blockers, &key)),
            dependents = escape_html(&joined_relations(&dependents, &key)),
        ));
    }

    let graph_data = serde_json::to_string(artifact).map_err(GraphError::EncodeArtifact)?;
    let graph_data = escape_script_data(&graph_data);
    let presentation_data =
        serde_json::to_string(presentation).map_err(GraphError::EncodeArtifact)?;
    let presentation_data = escape_script_data(&presentation_data);
    render_template(
        include_str!("render/index.html"),
        &[
            ("repository", escape_html(&artifact.repository)),
            ("synced_at", escape_html(&artifact.synced_at)),
            ("artifact_hash", escape_html(&artifact.artifact_hash)),
            ("graph_data", graph_data),
            ("presentation_data", presentation_data),
            (
                "project_header",
                if show_projects {
                    "<th scope=\"col\">Projects</th>".to_owned()
                } else {
                    String::new()
                },
            ),
            ("rows", rows),
        ],
    )
}
pub(super) fn stylesheet() -> &'static [u8] {
    include_bytes!("render/app.css")
}

pub(super) fn javascript() -> &'static [u8] {
    include_bytes!("render/app.js")
}

pub(super) fn network_view_javascript() -> &'static [u8] {
    include_bytes!("render/network-view.js")
}

pub(super) fn graph_query_javascript() -> &'static [u8] {
    include_bytes!("render/graph-query.js")
}

fn joined_relations(relations: &BTreeMap<String, Vec<String>>, key: &str) -> String {
    relations
        .get(key)
        .map(|values| values.join(", "))
        .unwrap_or_else(|| "—".to_owned())
}

fn render_template(template: &str, values: &[(&str, String)]) -> Result<String, GraphError> {
    let mut output = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        let placeholder = &remaining[start + 2..];
        let Some(end) = placeholder.find("}}") else {
            return Err(GraphError::InvalidHtmlTemplate(
                "unclosed placeholder".to_owned(),
            ));
        };
        let name = &placeholder[..end];
        let Some((_, value)) = values.iter().find(|(candidate, _)| *candidate == name) else {
            return Err(GraphError::InvalidHtmlTemplate(name.to_owned()));
        };
        output.push_str(value);
        remaining = &placeholder[end + 2..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn escape_script_data(value: &str) -> String {
    value
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}
