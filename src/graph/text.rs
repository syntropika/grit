pub(super) fn sort_and_deduplicate(values: &mut Vec<String>) {
    values.sort_by_key(|value| value.to_ascii_lowercase());
    values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
}

pub(super) fn strip_operation_markers(value: &str) -> String {
    let mut sanitized = value.to_owned();
    while let Some(start) = sanitized.find("<!-- grit:operation") {
        let Some(relative_end) = sanitized[start..].find("-->") else {
            sanitized.truncate(start);
            break;
        };
        sanitized.replace_range(start..start + relative_end + 3, "");
    }
    sanitized
}

pub(super) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
