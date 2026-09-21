use mockito::Matcher;

pub(crate) fn issue_delta_query(since: &str, page: Option<u64>) -> Matcher {
    let mut fields = vec![
        Matcher::UrlEncoded("state".into(), "all".into()),
        Matcher::UrlEncoded("sort".into(), "created".into()),
        Matcher::UrlEncoded("direction".into(), "asc".into()),
        Matcher::UrlEncoded("since".into(), since.into()),
        Matcher::UrlEncoded("per_page".into(), "100".into()),
    ];
    if let Some(page) = page {
        fields.push(Matcher::UrlEncoded("page".into(), page.to_string()));
    }
    Matcher::AllOf(fields)
}

#[allow(dead_code)]
pub(crate) fn comment_delta_query(since: &str, page: Option<u64>) -> Matcher {
    let mut fields = vec![
        Matcher::UrlEncoded("sort".into(), "created".into()),
        Matcher::UrlEncoded("direction".into(), "asc".into()),
        Matcher::UrlEncoded("since".into(), since.into()),
        Matcher::UrlEncoded("per_page".into(), "100".into()),
    ];
    if let Some(page) = page {
        fields.push(Matcher::UrlEncoded("page".into(), page.to_string()));
    }
    Matcher::AllOf(fields)
}
