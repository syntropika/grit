use std::{collections::BTreeSet, fs, path::Path};

use aho_corasick::AhoCorasick;

use super::{ConfirmedPublicRepository, PublicGraph, render, validation};
use crate::{
    graph::{GraphError, text::escape_html},
    model::{Actor, LocalReplica},
};

const PUBLIC_MANIFEST: [&str; 3] = ["graph.json", "graph.schema.json", "index.html"];

pub(super) fn prohibited_values(
    replica: &LocalReplica,
    artifact: &PublicGraph,
    repository: &ConfirmedPublicRepository,
) -> Result<BTreeSet<String>, GraphError> {
    let mut prohibited = BTreeSet::new();
    for issue in &replica.issues {
        collect(&mut prohibited, &issue.node_id);
        collect(&mut prohibited, &issue.url);
        collect(&mut prohibited, &issue.body);
        if issue.state.eq_ignore_ascii_case("closed") || issue.title != strip_markers(&issue.title)
        {
            collect(&mut prohibited, &issue.title);
        }
        collect_marker_payloads(&mut prohibited, &issue.title);
        collect_marker_payloads(&mut prohibited, &issue.body);
        if let Some(reason) = &issue.state_reason {
            collect(&mut prohibited, reason);
        }
        if let Some(author) = &issue.author {
            collect_actor(&mut prohibited, author);
        }
        for assignee in &issue.assignees {
            collect_actor(&mut prohibited, assignee);
        }
        for label in &issue.labels {
            collect(&mut prohibited, &label.name);
            collect_marker_payloads(&mut prohibited, &label.name);
            if let Some(node_id) = &label.node_id {
                collect(&mut prohibited, node_id);
            }
            if let Some(description) = &label.description {
                collect(&mut prohibited, description);
            }
        }
        for comment in &issue.comments {
            collect(&mut prohibited, &comment.node_id);
            collect(&mut prohibited, &comment.url);
            collect(&mut prohibited, &comment.body);
            collect_marker_payloads(&mut prohibited, &comment.body);
            if let Some(author) = &comment.author {
                collect_actor(&mut prohibited, author);
            }
        }
    }
    for dependency in &replica.dependencies {
        collect(&mut prohibited, &dependency.blocked.node_id);
        collect(&mut prohibited, &dependency.blocked.repository);
        collect(&mut prohibited, &dependency.blocker.repository);
        if let Some(node_id) = &dependency.blocker.node_id {
            collect(&mut prohibited, node_id);
        }
    }
    for (key, inventory) in &replica.relationships {
        for reference in std::iter::once(key)
            .chain(inventory.parent.iter())
            .chain(inventory.children.iter())
        {
            collect(&mut prohibited, reference);
            if let Some((owner, _)) = reference.split_once('#') {
                collect(&mut prohibited, owner);
            }
        }
    }
    let mut prohibited = expand_prohibited_values(prohibited);
    let intentional = intentional_public_patterns(artifact, repository)?;
    prohibited.retain(|pattern| !intentional.contains(pattern));
    Ok(prohibited)
}

pub(super) fn validate(
    directory: &Path,
    repository: &ConfirmedPublicRepository,
    prohibited_values: &BTreeSet<String>,
) -> Result<(), GraphError> {
    let prohibited_scanner = ProhibitedScanner::new(prohibited_values)?;
    let files = bundle_files(directory)?;
    if files != BTreeSet::from(PUBLIC_MANIFEST.map(str::to_owned)) {
        return Err(GraphError::UnsafePublicBundleFile(
            "manifest mismatch".to_owned(),
        ));
    }

    let graph_bytes = read_and_scan(directory, "graph.json", &prohibited_scanner)?;
    let artifact = validation::decode_and_validate(&graph_bytes, repository)?;

    let schema_bytes = read_and_scan(directory, "graph.schema.json", &prohibited_scanner)?;
    serde_json::from_slice::<serde_json::Value>(&schema_bytes).map_err(|error| {
        GraphError::InvalidPublicBundleJson("graph.schema.json".to_owned(), error)
    })?;
    if schema_bytes != render::schema_json(repository)? {
        return Err(GraphError::InvalidPublicBundleSchema);
    }

    let html_bytes = read_and_scan(directory, "index.html", &prohibited_scanner)?;
    if html_bytes != render::html(&artifact).into_bytes() {
        return Err(GraphError::UnsafePublicRuntime("index.html".to_owned()));
    }
    Ok(())
}

fn read_and_scan(
    directory: &Path,
    name: &str,
    prohibited_scanner: &ProhibitedScanner,
) -> Result<Vec<u8>, GraphError> {
    let bytes = fs::read(directory.join(name)).map_err(GraphError::InspectPublicBundle)?;
    scan_bytes(name, &bytes, prohibited_scanner)?;
    Ok(bytes)
}

fn bundle_files(directory: &Path) -> Result<BTreeSet<String>, GraphError> {
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(GraphError::InspectPublicBundle)? {
        let entry = entry.map_err(GraphError::InspectPublicBundle)?;
        let file_type = entry.file_type().map_err(GraphError::InspectPublicBundle)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| GraphError::UnsafePublicBundleFile(name.to_string_lossy().into()))?;
        if !file_type.is_file() || !PUBLIC_MANIFEST.contains(&name.as_str()) {
            return Err(GraphError::UnsafePublicBundleFile(name));
        }
        files.insert(name);
    }
    Ok(files)
}

fn scan_bytes(
    name: &str,
    bytes: &[u8],
    prohibited_scanner: &ProhibitedScanner,
) -> Result<(), GraphError> {
    if prohibited_scanner.is_match(bytes) {
        return Err(GraphError::ProhibitedPublicValue(name.to_owned()));
    }
    if contains_secret_pattern(bytes) {
        return Err(GraphError::PublicSecretPattern(name.to_owned()));
    }
    Ok(())
}

struct ProhibitedScanner(Option<AhoCorasick>);

impl ProhibitedScanner {
    fn new(patterns: &BTreeSet<String>) -> Result<Self, GraphError> {
        if patterns.is_empty() {
            return Ok(Self(None));
        }
        AhoCorasick::new(patterns)
            .map(|scanner| Self(Some(scanner)))
            .map_err(GraphError::CompilePublicScanner)
    }

    fn is_match(&self, bytes: &[u8]) -> bool {
        self.0
            .as_ref()
            .is_some_and(|scanner| scanner.is_match(bytes))
    }
}

fn contains_secret_pattern(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let lower = text.to_ascii_lowercase();
    if [
        "-----begin private key-----",
        "-----begin rsa private key-----",
        "authorization: bearer ",
        "aws_secret_access_key",
        "\"access_token\"",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
    {
        return true;
    }
    [
        ("github_pat_", 20),
        ("ghp_", 20),
        ("gho_", 20),
        ("ghu_", 20),
        ("ghs_", 20),
        ("ghr_", 20),
    ]
    .iter()
    .any(|(prefix, minimum)| contains_token(&text, prefix, *minimum))
        || contains_aws_key(&text)
}

fn contains_token(text: &str, prefix: &str, minimum: usize) -> bool {
    let mut remaining = text;
    while let Some(start) = remaining.find(prefix) {
        let tail = &remaining[start + prefix.len()..];
        if tail
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count()
            >= minimum
        {
            return true;
        }
        remaining = &tail[tail.len().min(1)..];
    }
    false
}

fn contains_aws_key(text: &str) -> bool {
    text.as_bytes().windows(20).any(|window| {
        window.starts_with(b"AKIA")
            && window[4..]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    })
}

fn collect(values: &mut BTreeSet<String>, value: &str) {
    if !value.is_empty() {
        values.insert(value.to_owned());
    }
}

fn expand_prohibited_values(values: BTreeSet<String>) -> BTreeSet<String> {
    let mut expanded = BTreeSet::new();
    for value in values {
        let json = serde_json::to_string(&value).expect("strings always serialize");
        expanded.insert(json);
        expanded.insert(format!(">{}<", escape_html(&value)));
    }
    expanded
}

fn intentional_public_patterns(
    artifact: &PublicGraph,
    repository: &ConfirmedPublicRepository,
) -> Result<BTreeSet<String>, GraphError> {
    let artifact_json = serde_json::to_value(artifact).map_err(GraphError::EncodeArtifact)?;
    let schema_json: serde_json::Value = serde_json::from_slice(&render::schema_json(repository)?)
        .map_err(GraphError::ValidateSchema)?;
    let mut strings = BTreeSet::new();
    collect_json_strings(&artifact_json, &mut strings);
    collect_json_strings(&schema_json, &mut strings);
    let mut patterns = strings
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("strings always serialize"))
        .collect::<BTreeSet<_>>();

    let html = render::html(artifact);
    let mut remaining = html.as_str();
    while let Some(start) = remaining.find('>') {
        let text = &remaining[start + 1..];
        let Some(end) = text.find('<') else {
            break;
        };
        if !text[..end].is_empty() {
            patterns.insert(format!(">{}<", &text[..end]));
        }
        remaining = &text[end..];
    }
    Ok(patterns)
}

fn collect_json_strings(value: &serde_json::Value, strings: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(value) => {
            strings.insert(value.clone());
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_json_strings(value, strings);
            }
        }
        serde_json::Value::Object(values) => {
            strings.extend(values.keys().cloned());
            for value in values.values() {
                collect_json_strings(value, strings);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn collect_actor(values: &mut BTreeSet<String>, actor: &Actor) {
    collect(values, &actor.node_id);
    collect(values, &actor.login);
}

fn collect_marker_payloads(values: &mut BTreeSet<String>, text: &str) {
    for prefix in [
        "<!-- hyfa:operation",
        "<!-- hyfa-operation:",
        "<!-- grit:operation",
        "<!-- grit-operation:",
    ] {
        let mut remaining = text;
        while let Some(start) = remaining.find(prefix) {
            let payload = &remaining[start + prefix.len()..];
            let Some(end) = payload.find("-->") else {
                break;
            };
            collect(values, payload[..end].trim());
            remaining = &payload[end + 3..];
        }
    }
}

fn strip_markers(value: &str) -> String {
    crate::graph::text::strip_operation_markers(value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        BTreeSet, ProhibitedScanner, bundle_files, contains_secret_pattern,
        expand_prohibited_values, scan_bytes,
    };

    #[test]
    fn secret_patterns_cover_github_aws_and_private_keys_without_matching_plain_words() {
        assert!(contains_secret_pattern(
            b"github_pat_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        ));
        assert!(contains_secret_pattern(b"AKIA1234567890ABCDEF"));
        assert!(contains_secret_pattern(b"-----BEGIN PRIVATE KEY-----"));
        assert!(!contains_secret_pattern(b"token handling and GitHub Pages"));
    }

    #[test]
    fn byte_scan_rejects_prohibited_values_without_disclosing_them() {
        let prohibited = expand_prohibited_values(BTreeSet::from([
            "private fixture".to_owned(),
            "customer \"<alpha>\"".to_owned(),
            "alice".to_owned(),
        ]));
        let scanner = ProhibitedScanner::new(&prohibited).expect("compiled scanner");
        for bytes in [
            b"<td>private fixture</td>".as_slice(),
            br#"{"leak": "customer \"<alpha>\""}"#,
            b"<td>alice</td>",
        ] {
            let error = scan_bytes("index.html", bytes, &scanner).expect_err("prohibited value");
            assert!(!error.to_string().contains("private fixture"));
            assert!(!error.to_string().contains("customer"));
            assert!(!error.to_string().contains("alice"));
        }
    }

    #[test]
    fn prohibited_value_scanner_handles_repository_scale_in_one_automaton() {
        let patterns = (0..20_000)
            .map(|index| format!("private-fixture-{index:05}"))
            .collect::<BTreeSet<_>>();
        let scanner = ProhibitedScanner::new(&patterns).expect("compiled scanner");
        let mut bundle = vec![b'x'; 1_000_000];
        bundle.extend_from_slice(b"private-fixture-19999");
        assert!(scanner.is_match(&bundle));
    }

    #[test]
    fn manifest_rejects_nested_unknown_and_debug_outputs() {
        for (name, contents) in [
            ("nested", ""),
            ("app.js.map", "debug"),
            ("debug.log", "debug"),
            ("service-worker.js", "debug"),
            ("app.js", "fetch (\"https://api.example\")"),
            ("app.css", "@import \"https://cdn.example/x.css\""),
        ] {
            let directory = TempDir::new().expect("bundle directory");
            let path = directory.path().join(name);
            if name == "nested" {
                fs::create_dir(&path).expect("nested directory");
            } else {
                fs::write(&path, contents).expect("unsafe bundle file");
            }
            assert!(bundle_files(directory.path()).is_err(), "accepted {name}");
        }
    }
}
