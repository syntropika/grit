use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{
        ACCEPT, AUTHORIZATION, ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH, LINK, USER_AGENT,
    },
};
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

use crate::{
    auth::AuthToken,
    model::{
        Actor, BlockerIdentity, BlockerScope, Comment, Dependency, Issue, IssueIdentity, Label,
        LocalReplica, OrdinaryIssueCursor, SyncMetadata,
    },
    repository::Repository,
};

const API_VERSION: &str = "2026-03-10";

pub(crate) struct GitHubClient {
    client: Client,
    base_url: Url,
}

pub(crate) struct RepositoryData {
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) sync: SyncMetadata,
}

impl GitHubClient {
    pub(crate) fn new(base_url: Url, token: &AuthToken) -> Result<Self, GitHubError> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
            .map_err(|_| GitHubError::InvalidToken)?;
        authorization.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("grit/0.1"));
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static(API_VERSION),
        );

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .map_err(GitHubError::BuildClient)?;

        Ok(Self { client, base_url })
    }

    pub(crate) fn fetch_repository(
        &self,
        repository: &Repository,
        previous: Option<&LocalReplica>,
    ) -> Result<RepositoryData, GitHubError> {
        if let Some(previous) = previous
            && let Some(cursor) = previous.sync.ordinary_issues.as_ref()
        {
            return self.fetch_incremental(repository, previous, cursor);
        }
        self.fetch_full(repository)
    }

    fn fetch_full(&self, repository: &Repository) -> Result<RepositoryData, GitHubError> {
        let owner = repository.owner();
        let repo = repository.name();
        let issue_url = self.endpoint(&format!("repos/{owner}/{repo}/issues"))?;
        let raw_issues: Vec<GitHubIssue> = self.paginate(
            issue_url,
            &[
                ("state", "all"),
                ("sort", "created"),
                ("direction", "asc"),
                ("per_page", "100"),
            ],
        )?;

        let mut issues: Vec<Issue> = raw_issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(GitHubIssue::normalize)
            .collect();
        issues.sort_by_key(|issue| (issue.number, issue.id));

        let comments_url = self.endpoint(&format!("repos/{owner}/{repo}/issues/comments"))?;
        let raw_comments: Vec<GitHubComment> =
            self.paginate(comments_url, &[("per_page", "100")])?;
        attach_comments(&mut issues, raw_comments);

        let mut dependencies = BTreeMap::new();
        for issue in &issues {
            let dependency_url = self.endpoint(&format!(
                "repos/{owner}/{repo}/issues/{}/dependencies/blocked_by",
                issue.number
            ))?;
            let blockers: Vec<GitHubBlocker> =
                self.paginate(dependency_url, &[("per_page", "100")])?;
            for blocker in blockers {
                let dependency = normalize_dependency(repository.full_name(), issue, blocker)?;
                dependencies
                    .entry(DependencyKey::from(&dependency))
                    .or_insert(dependency);
            }
        }

        let watermark = latest_watermark(&issues)?
            .unwrap_or_else(|| Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        Ok(RepositoryData {
            issues,
            dependencies: dependencies.into_values().collect(),
            sync: SyncMetadata {
                ordinary_issues: Some(OrdinaryIssueCursor {
                    watermark,
                    issues_etag: None,
                    comments_etag: None,
                }),
            },
        })
    }

    fn fetch_incremental(
        &self,
        repository: &Repository,
        previous: &LocalReplica,
        cursor: &OrdinaryIssueCursor,
    ) -> Result<RepositoryData, GitHubError> {
        let owner = repository.owner();
        let repo = repository.name();
        let since = overlapped_since(&cursor.watermark)?;

        let issue_url = self.endpoint(&format!("repos/{owner}/{repo}/issues"))?;
        let issue_delta: ConditionalPages<GitHubIssue> = self.paginate_conditional(
            issue_url,
            &[
                ("state", "all"),
                ("sort", "updated"),
                ("direction", "asc"),
                ("since", since.as_str()),
                ("per_page", "100"),
            ],
            cursor.issues_etag.as_deref(),
        )?;

        let comments_url = self.endpoint(&format!("repos/{owner}/{repo}/issues/comments"))?;
        let comment_delta: ConditionalPages<GitHubComment> = self.paginate_conditional(
            comments_url,
            &[
                ("sort", "updated"),
                ("direction", "asc"),
                ("since", since.as_str()),
                ("per_page", "100"),
            ],
            cursor.comments_etag.as_deref(),
        )?;

        let mut issues = previous.issues.clone();
        let mut changed_issue_numbers = BTreeSet::new();
        let issue_etag = match issue_delta {
            ConditionalPages::NotModified => cursor.issues_etag.clone(),
            ConditionalPages::Modified(page) => {
                for raw in page
                    .items
                    .into_iter()
                    .filter(|issue| issue.pull_request.is_none())
                {
                    let mut incoming = raw.normalize();
                    match issues.iter().position(|issue| issue.id == incoming.id) {
                        Some(index) => {
                            incoming.comments = issues[index].comments.clone();
                            if issues[index] != incoming {
                                changed_issue_numbers.insert(incoming.number);
                                issues[index] = incoming;
                            }
                        }
                        None => {
                            changed_issue_numbers.insert(incoming.number);
                            issues.push(incoming);
                        }
                    }
                }
                page.safe_etag
            }
        };

        let comments_etag = match comment_delta {
            ConditionalPages::NotModified => cursor.comments_etag.clone(),
            ConditionalPages::Modified(page) => {
                attach_comments(&mut issues, page.items);
                page.safe_etag
            }
        };
        issues.sort_by_key(|issue| (issue.number, issue.id));

        let mut dependencies: BTreeMap<_, _> = previous
            .dependencies
            .iter()
            .cloned()
            .map(|dependency| (DependencyKey::from(&dependency), dependency))
            .collect();
        for number in &changed_issue_numbers {
            dependencies.retain(|key, _| key.blocked_number != *number);
            let issue = issues
                .iter()
                .find(|issue| issue.number == *number)
                .expect("changed Issue remains in the merged inventory");
            let dependency_url = self.endpoint(&format!(
                "repos/{owner}/{repo}/issues/{number}/dependencies/blocked_by"
            ))?;
            let blockers: Vec<GitHubBlocker> =
                self.paginate(dependency_url, &[("per_page", "100")])?;
            for blocker in blockers {
                let dependency = normalize_dependency(repository.full_name(), issue, blocker)?;
                dependencies
                    .entry(DependencyKey::from(&dependency))
                    .or_insert(dependency);
            }
        }

        let watermark = latest_watermark(&issues)?
            .map(|latest| later_timestamp(&cursor.watermark, &latest))
            .transpose()?
            .unwrap_or_else(|| cursor.watermark.clone());
        let watermark_advanced = watermark != cursor.watermark;

        Ok(RepositoryData {
            issues,
            dependencies: dependencies.into_values().collect(),
            sync: SyncMetadata {
                ordinary_issues: Some(OrdinaryIssueCursor {
                    watermark,
                    issues_etag: (!watermark_advanced).then_some(issue_etag).flatten(),
                    comments_etag: (!watermark_advanced).then_some(comments_etag).flatten(),
                }),
            },
        })
    }

    fn paginate<T>(&self, url: Url, initial_query: &[(&str, &str)]) -> Result<Vec<T>, GitHubError>
    where
        T: DeserializeOwned,
    {
        match self.paginate_conditional(url, initial_query, None)? {
            ConditionalPages::Modified(page) => Ok(page.items),
            ConditionalPages::NotModified => Err(GitHubError::UnexpectedNotModified),
        }
    }

    fn paginate_conditional<T>(
        &self,
        mut url: Url,
        initial_query: &[(&str, &str)],
        etag: Option<&str>,
    ) -> Result<ConditionalPages<T>, GitHubError>
    where
        T: DeserializeOwned,
    {
        url.query_pairs_mut()
            .extend_pairs(initial_query.iter().copied());
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        let mut first_response_etag = None;
        let mut first_page_len = 0;
        let mut first_page_had_next = false;

        loop {
            let first_page = visited.is_empty();
            if !visited.insert(url.as_str().to_owned()) {
                return Err(GitHubError::PaginationLoop);
            }
            self.require_same_origin(&url)?;

            let mut request = self.client.get(url.clone());
            if first_page && let Some(etag) = etag {
                let value = HeaderValue::from_str(etag).map_err(|_| GitHubError::InvalidEtag)?;
                request = request.header(IF_NONE_MATCH, value);
            }
            let response = request.send().map_err(GitHubError::Request)?;
            if first_page && etag.is_some() && response.status() == StatusCode::NOT_MODIFIED {
                return Ok(ConditionalPages::NotModified);
            }
            if first_page {
                first_response_etag = response
                    .headers()
                    .get(ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
            }
            let (page, next) = self.decode_page(response)?;
            if first_page {
                first_page_len = page.len();
                first_page_had_next = next.is_some();
            }
            results.extend(page);
            match next {
                Some(next) => url = next,
                None => {
                    let safe_etag = (!first_page_had_next && first_page_len < 100)
                        .then_some(first_response_etag)
                        .flatten();
                    return Ok(ConditionalPages::Modified(CompletePages {
                        items: results,
                        safe_etag,
                    }));
                }
            }
        }
    }

    fn decode_page<T>(&self, response: Response) -> Result<(Vec<T>, Option<Url>), GitHubError>
    where
        T: DeserializeOwned,
    {
        let status = response.status();
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }

        let next = next_link(response.headers())?;
        let page = response.json().map_err(GitHubError::Decode)?;
        Ok((page, next))
    }

    fn endpoint(&self, path: &str) -> Result<Url, GitHubError> {
        self.base_url
            .join(path)
            .map_err(|source| GitHubError::InvalidUrl { source })
    }

    fn require_same_origin(&self, candidate: &Url) -> Result<(), GitHubError> {
        if self.base_url.scheme() != candidate.scheme()
            || self.base_url.host_str() != candidate.host_str()
            || self.base_url.port_or_known_default() != candidate.port_or_known_default()
        {
            return Err(GitHubError::CrossOriginPagination);
        }
        Ok(())
    }
}

enum ConditionalPages<T> {
    NotModified,
    Modified(CompletePages<T>),
}

struct CompletePages<T> {
    items: Vec<T>,
    safe_etag: Option<String>,
}

fn overlapped_since(watermark: &str) -> Result<String, GitHubError> {
    let watermark = DateTime::parse_from_rfc3339(watermark)
        .map_err(|_| GitHubError::InvalidTimestamp(watermark.to_owned()))?;
    Ok((watermark - Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn latest_watermark(issues: &[Issue]) -> Result<Option<String>, GitHubError> {
    let timestamps = issues.iter().flat_map(|issue| {
        std::iter::once(issue.updated_at.as_str()).chain(
            issue
                .comments
                .iter()
                .map(|comment| comment.updated_at.as_str()),
        )
    });
    let mut latest = None;
    for value in timestamps {
        let parsed = DateTime::parse_from_rfc3339(value)
            .map_err(|_| GitHubError::InvalidTimestamp(value.to_owned()))?;
        if latest.as_ref().is_none_or(|current| parsed > *current) {
            latest = Some(parsed);
        }
    }
    Ok(latest.map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true)))
}

fn later_timestamp(left: &str, right: &str) -> Result<String, GitHubError> {
    let left_parsed = DateTime::parse_from_rfc3339(left)
        .map_err(|_| GitHubError::InvalidTimestamp(left.to_owned()))?;
    let right_parsed = DateTime::parse_from_rfc3339(right)
        .map_err(|_| GitHubError::InvalidTimestamp(right.to_owned()))?;
    Ok(if right_parsed > left_parsed {
        right.to_owned()
    } else {
        left.to_owned()
    })
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct DependencyKey {
    blocked_number: u64,
    blocker_repository: String,
    blocker_number: u64,
}

impl From<&Dependency> for DependencyKey {
    fn from(dependency: &Dependency) -> Self {
        Self {
            blocked_number: dependency.blocked.number,
            blocker_repository: dependency.blocker.repository.to_ascii_lowercase(),
            blocker_number: dependency.blocker.number,
        }
    }
}

fn attach_comments(issues: &mut [Issue], comments: Vec<GitHubComment>) {
    let by_number: BTreeMap<u64, usize> = issues
        .iter()
        .enumerate()
        .map(|(index, issue)| (issue.number, index))
        .collect();

    for comment in comments {
        if let Some(number) = issue_number_from_url(&comment.issue_url)
            && let Some(index) = by_number.get(&number)
        {
            let comment = comment.normalize();
            match issues[*index]
                .comments
                .iter()
                .position(|existing| existing.id == comment.id)
            {
                Some(comment_index) => issues[*index].comments[comment_index] = comment,
                None => issues[*index].comments.push(comment),
            }
        }
    }
    for issue in issues {
        issue.comments.sort_by_key(|comment| comment.id);
    }
}

fn issue_number_from_url(value: &str) -> Option<u64> {
    Url::parse(value)
        .ok()?
        .path_segments()?
        .next_back()?
        .parse()
        .ok()
}

fn normalize_dependency(
    repository: &str,
    blocked: &Issue,
    blocker: GitHubBlocker,
) -> Result<Dependency, GitHubError> {
    let blocker_repository = repository_from_api_url(&blocker.repository_url)?;
    let internal = blocker_repository.eq_ignore_ascii_case(repository);
    Ok(Dependency {
        blocked: IssueIdentity {
            repository: repository.to_owned(),
            number: blocked.number,
            id: blocked.id,
            node_id: blocked.node_id.clone(),
        },
        blocker: BlockerIdentity {
            repository: blocker_repository,
            number: blocker.number,
            state: blocker.state,
            scope: if internal {
                BlockerScope::Internal
            } else {
                BlockerScope::External
            },
            id: internal.then_some(blocker.id),
            node_id: internal.then_some(blocker.node_id),
        },
    })
}

fn repository_from_api_url(value: &str) -> Result<String, GitHubError> {
    let parsed = Url::parse(value).map_err(|source| GitHubError::InvalidUrl { source })?;
    let segments: Vec<_> = parsed
        .path_segments()
        .ok_or(GitHubError::InvalidRepositoryUrl)?
        .collect();
    let repos = segments
        .iter()
        .position(|segment| *segment == "repos")
        .ok_or(GitHubError::InvalidRepositoryUrl)?;
    let owner = segments.get(repos + 1).filter(|value| !value.is_empty());
    let repo = segments.get(repos + 2).filter(|value| !value.is_empty());
    match (owner, repo) {
        (Some(owner), Some(repo)) => Ok(format!("{owner}/{repo}")),
        _ => Err(GitHubError::InvalidRepositoryUrl),
    }
}

fn next_link(headers: &HeaderMap) -> Result<Option<Url>, GitHubError> {
    let Some(value) = headers.get(LINK) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| GitHubError::InvalidLink)?;
    for part in value.split(',') {
        let mut pieces = part.trim().split(';');
        let target = pieces.next().ok_or(GitHubError::InvalidLink)?.trim();
        let is_next = pieces.any(|piece| piece.trim() == "rel=\"next\"");
        if is_next {
            let target = target
                .strip_prefix('<')
                .and_then(|url| url.strip_suffix('>'))
                .ok_or(GitHubError::InvalidLink)?;
            return Url::parse(target)
                .map(Some)
                .map_err(|source| GitHubError::InvalidUrl { source });
        }
    }
    Ok(None)
}

fn api_status_error(status: StatusCode, headers: &HeaderMap) -> GitHubError {
    let remaining = headers
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok());
    let reset = headers
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok());
    let retry_after = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok());

    if status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::FORBIDDEN && (remaining == Some("0") || retry_after.is_some())
    {
        return GitHubError::RateLimited {
            reset: reset.map(str::to_owned),
            retry_after: retry_after.map(str::to_owned),
        };
    }

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => GitHubError::Authentication(status),
        _ => GitHubError::Status(status),
    }
}

#[derive(Deserialize)]
struct GitHubIssue {
    id: u64,
    node_id: String,
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    state: String,
    state_reason: Option<String>,
    user: Option<GitHubActor>,
    #[serde(default)]
    assignees: Vec<GitHubActor>,
    #[serde(default)]
    labels: Vec<GitHubLabel>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    pull_request: Option<serde_json::Value>,
}

impl GitHubIssue {
    fn normalize(self) -> Issue {
        let mut assignees: Vec<_> = self
            .assignees
            .into_iter()
            .map(GitHubActor::normalize)
            .collect();
        assignees.sort();
        let mut labels: Vec<_> = self
            .labels
            .into_iter()
            .map(GitHubLabel::normalize)
            .collect();
        labels.sort();
        Issue {
            id: self.id,
            node_id: self.node_id,
            number: self.number,
            url: self.html_url,
            title: self.title,
            body: self.body.unwrap_or_default(),
            state: self.state,
            state_reason: self.state_reason,
            author: self.user.map(GitHubActor::normalize),
            assignees,
            labels,
            comments: Vec::new(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
        }
    }
}

#[derive(Deserialize)]
struct GitHubActor {
    id: u64,
    node_id: String,
    login: String,
}

impl GitHubActor {
    fn normalize(self) -> Actor {
        Actor {
            id: self.id,
            node_id: self.node_id,
            login: self.login,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GitHubLabel {
    Detailed {
        id: u64,
        node_id: String,
        name: String,
        color: String,
        description: Option<String>,
    },
    Name(String),
}

impl GitHubLabel {
    fn normalize(self) -> Label {
        match self {
            Self::Detailed {
                id,
                node_id,
                name,
                color,
                description,
            } => Label {
                id: Some(id),
                node_id: Some(node_id),
                name,
                color: Some(color),
                description,
            },
            Self::Name(name) => Label {
                id: None,
                node_id: None,
                name,
                color: None,
                description: None,
            },
        }
    }
}

#[derive(Deserialize)]
struct GitHubComment {
    id: u64,
    node_id: String,
    html_url: String,
    body: Option<String>,
    user: Option<GitHubActor>,
    #[serde(default)]
    author_association: String,
    created_at: String,
    updated_at: String,
    issue_url: String,
}

impl GitHubComment {
    fn normalize(self) -> Comment {
        Comment {
            id: self.id,
            node_id: self.node_id,
            url: self.html_url,
            body: self.body.unwrap_or_default(),
            author: self.user.map(GitHubActor::normalize),
            author_association: self.author_association,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct GitHubBlocker {
    id: u64,
    node_id: String,
    repository_url: String,
    number: u64,
    state: String,
}

#[derive(Debug, Error)]
pub(crate) enum GitHubError {
    #[error("the GitHub token cannot be represented as an HTTP header")]
    InvalidToken,
    #[error("could not build the GitHub HTTP client: {0}")]
    BuildClient(reqwest::Error),
    #[error("GitHub request failed: {0}")]
    Request(reqwest::Error),
    #[error("GitHub returned HTTP {0}")]
    Status(StatusCode),
    #[error("GitHub rejected authentication with HTTP {0}")]
    Authentication(StatusCode),
    #[error("GitHub rate limit reached (reset={reset:?}, retry_after={retry_after:?})")]
    RateLimited {
        reset: Option<String>,
        retry_after: Option<String>,
    },
    #[error("GitHub returned invalid JSON for a paginated response: {0}")]
    Decode(reqwest::Error),
    #[error("GitHub returned 304 Not Modified without a scoped conditional request")]
    UnexpectedNotModified,
    #[error("the stored GitHub ETag cannot be represented as an HTTP header")]
    InvalidEtag,
    #[error("GitHub returned an invalid updated_at timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("GitHub returned an invalid pagination Link header")]
    InvalidLink,
    #[error("GitHub pagination attempted to revisit a page")]
    PaginationLoop,
    #[error("GitHub pagination attempted to send credentials to another origin")]
    CrossOriginPagination,
    #[error("invalid GitHub URL: {source}")]
    InvalidUrl { source: url::ParseError },
    #[error("a dependency contained an invalid repository URL")]
    InvalidRepositoryUrl,
}
