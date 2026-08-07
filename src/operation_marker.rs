const PREFIX: &str = "<!-- grit-operation:";
const SUFFIX: &str = " -->";
const UUID_LENGTH: usize = 36;

pub(crate) fn embed(body: &str, marker: &str) -> String {
    let comment = format!("{PREFIX}{marker}{SUFFIX}");
    if body.is_empty() {
        comment
    } else {
        format!("{body}\n\n{comment}")
    }
}

pub(crate) fn values(body: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut search_from = 0;
    while let Some(relative_start) = body[search_from..].find(PREFIX) {
        let start = search_from + relative_start;
        let value_start = start + PREFIX.len();
        let value_end = value_start.saturating_add(UUID_LENGTH);
        let Some(value) = body.get(value_start..value_end) else {
            search_from = value_start;
            continue;
        };
        if !body
            .get(value_end..)
            .is_some_and(|tail| tail.starts_with(SUFFIX))
        {
            search_from = value_start;
            continue;
        }
        if uuid::Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value) {
            values.push(value);
        }
        search_from = value_end + SUFFIX.len();
    }
    values
}

pub(crate) fn strip(body: &str) -> String {
    let mut visible = body.to_owned();
    let mut search_from = 0;
    while let Some(relative_start) = visible[search_from..].find(PREFIX) {
        let start = search_from + relative_start;
        let value_start = start + PREFIX.len();
        let value_end = value_start.saturating_add(UUID_LENGTH);
        let Some(value) = visible.get(value_start..value_end) else {
            search_from = value_start;
            continue;
        };
        if !visible
            .get(value_end..)
            .is_some_and(|tail| tail.starts_with(SUFFIX))
        {
            search_from = value_start;
            continue;
        }
        if !uuid::Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value) {
            search_from = value_end + SUFFIX.len();
            continue;
        }
        let end = value_end + SUFFIX.len();
        let removal_start = visible[..start]
            .strip_suffix("\n\n")
            .map_or(start, |prefix| prefix.len());
        visible.replace_range(removal_start..end, "");
        search_from = removal_start;
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::{embed, strip, values};

    #[test]
    fn markers_round_trip_without_changing_visible_markdown() {
        let body = "Visible <!-- ordinary --> Markdown";
        let marker = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let embedded = embed(body, marker);
        assert!(embedded.contains(marker));
        assert_eq!(values(&embedded), [marker]);
        assert_eq!(strip(&embedded), body);
    }

    #[test]
    fn malformed_or_noncanonical_marker_like_comments_remain_visible() {
        let body = concat!(
            "before <!-- grit-operation:not-a-uuid --> middle ",
            "<!-- grit-operation:6BA7B810-9DAD-11D1-80B4-00C04FD430C8 --> after ",
            "<!-- grit-operation:unterminated"
        );
        assert!(values(body).is_empty());
        assert_eq!(strip(body), body);

        let marker = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let embedded = embed(body, marker);
        assert_eq!(strip(&embedded), body);
    }
}
