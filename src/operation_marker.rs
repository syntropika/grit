use std::collections::{BTreeMap, BTreeSet};

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

pub(crate) enum Recovery<T> {
    Missing,
    Unique(T),
    Ambiguous(usize),
}

pub(crate) fn classify<T: Clone>(matches: Option<&[T]>) -> Recovery<T> {
    match matches.unwrap_or_default() {
        [] => Recovery::Missing,
        [identity] => Recovery::Unique(identity.clone()),
        identities => Recovery::Ambiguous(identities.len()),
    }
}

pub(crate) fn index<T, U>(
    items: impl IntoIterator<Item = T>,
    markers: &BTreeSet<String>,
    body: impl for<'a> Fn(&'a T) -> Option<&'a str>,
    identity: impl Fn(&T) -> Option<U>,
) -> BTreeMap<String, Vec<U>>
where
    U: Clone,
{
    let mut matches = BTreeMap::<String, Vec<U>>::new();
    for item in items {
        let Some(body) = body(&item) else {
            continue;
        };
        let matched_markers = values(body)
            .into_iter()
            .filter(|marker| markers.contains(*marker))
            .collect::<BTreeSet<_>>();
        if matched_markers.is_empty() {
            continue;
        }
        let Some(identity) = identity(&item) else {
            continue;
        };
        for marker in matched_markers {
            matches
                .entry(marker.to_owned())
                .or_default()
                .push(identity.clone());
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Recovery, classify, embed, index, strip, values};

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

    #[test]
    fn marker_matches_share_one_index_and_cardinality_policy() {
        let marker = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let duplicate_body = embed(&embed("same remote object", marker), marker);
        let duplicate_matches = index(
            [(0, duplicate_body)],
            &BTreeSet::from([marker.to_owned()]),
            |(_, body)| Some(body.as_str()),
            |(index, _)| Some(*index),
        );
        assert!(matches!(
            classify(duplicate_matches.get(marker).map(Vec::as_slice)),
            Recovery::Unique(0)
        ));

        let bodies = [embed("first", marker), embed("second", marker)];
        let matches = index(
            bodies.iter().enumerate(),
            &BTreeSet::from([marker.to_owned()]),
            |(_, body)| Some(body.as_str()),
            |(index, _)| Some(*index),
        );
        assert!(matches!(
            classify(matches.get(marker).map(Vec::as_slice)),
            Recovery::Ambiguous(2)
        ));
        assert!(matches!(classify::<usize>(None), Recovery::Missing));
    }
}
