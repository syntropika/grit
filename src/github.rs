use std::collections::{BTreeMap, HashSet};

use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LINK, USER_AGENT},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

use crate::{
    auth::AuthToken,
    model::{
        Actor, BlockerIdentity, BlockerScope, Comment, Dependency, Issue, IssueIdentity, Label,
    },
    repository::{IssueReference, Repository},
};

const API_VERSION: &str = "2026-03-10";

pub(crate) struct GitHubClient {
    client: Client,
    base_url: Url,
}

pub(crate) struct RepositoryData {
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
}

#[derive(Clone, Copy)]
pub(crate) enum DependencyIntent {
    Block,
    Unblock,
}

impl DependencyIntent {
    pub(crate) fn desired_present(self) -> bool {
        matches!(self, Self::Block)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DependencyChange {
    Created,
    AlreadyPresent,
    Removed,
    AlreadyAbsent,
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
    ) -> Result<RepositoryData, GitHubError> {
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
            let blockers = self.fetch_blockers(repository, issue.number, "100")?;
            for blocker in blockers {
                let dependency = normalize_dependency(repository.full_name(), issue, blocker)?;
                dependencies
                    .entry(DependencyKey::from(&dependency))
                    .or_insert(dependency);
            }
        }

        Ok(RepositoryData {
            issues,
            dependencies: dependencies.into_values().collect(),
        })
    }

    pub(crate) fn mutate_dependency(
        &self,
        blocked: &IssueReference,
        blocker: &IssueReference,
        intent: DependencyIntent,
    ) -> Result<DependencyChange, GitHubError> {
        let blocker_id = self.fetch_issue_id(blocker)?;
        let blockers = self.fetch_blockers(blocked.repository(), blocked.number(), "50")?;
        let present = blockers.iter().any(|candidate| candidate.id == blocker_id);

        match (intent, present) {
            (DependencyIntent::Block, true) => Ok(DependencyChange::AlreadyPresent),
            (DependencyIntent::Unblock, false) => Ok(DependencyChange::AlreadyAbsent),
            (DependencyIntent::Block, false) => self.add_dependency(blocked, blocker_id),
            (DependencyIntent::Unblock, true) => self.remove_dependency(blocked, blocker_id),
        }
    }

    fn fetch_issue_id(&self, issue: &IssueReference) -> Result<u64, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/{}",
            issue.repository().owner(),
            issue.repository().name(),
            issue.number()
        ))?;
        let response = self.client.get(url).send().map_err(GitHubError::Request)?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }
        let remote: GitHubIssueLocator = response.json().map_err(GitHubError::Decode)?;
        if remote.number != issue.number() {
            return Err(GitHubError::IssueIdentityMismatch);
        }
        if remote.pull_request.is_some() {
            return Err(GitHubError::PullRequestDependency(issue.stable_key()));
        }
        Ok(remote.id)
    }

    fn fetch_blockers(
        &self,
        repository: &Repository,
        issue_number: u64,
        per_page: &str,
    ) -> Result<Vec<GitHubBlocker>, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/{issue_number}/dependencies/blocked_by",
            repository.owner(),
            repository.name()
        ))?;
        self.paginate(url, &[("per_page", per_page)])
    }

    fn add_dependency(
        &self,
        blocked: &IssueReference,
        blocker_id: u64,
    ) -> Result<DependencyChange, GitHubError> {
        let url = self.dependency_url(blocked)?;
        let response = self
            .client
            .post(url)
            .json(&AddDependency {
                issue_id: blocker_id,
            })
            .send()
            .map_err(|source| GitHubError::MutationUncertain {
                operation: "adding the blocked-by relationship",
                source,
            })?;
        let status = response.status();
        if status == StatusCode::CREATED {
            return Ok(DependencyChange::Created);
        }
        if status == StatusCode::UNPROCESSABLE_ENTITY
            && self.dependency_exists(blocked, blocker_id)?
        {
            return Ok(DependencyChange::AlreadyPresent);
        }
        Err(mutation_status_error(
            status,
            response.headers(),
            "adding the blocked-by relationship",
        ))
    }

    fn remove_dependency(
        &self,
        blocked: &IssueReference,
        blocker_id: u64,
    ) -> Result<DependencyChange, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/{}/dependencies/blocked_by/{blocker_id}",
            blocked.repository().owner(),
            blocked.repository().name(),
            blocked.number()
        ))?;
        let response =
            self.client
                .delete(url)
                .send()
                .map_err(|source| GitHubError::MutationUncertain {
                    operation: "removing the blocked-by relationship",
                    source,
                })?;
        let status = response.status();
        if status.is_success() {
            return Ok(DependencyChange::Removed);
        }
        if status == StatusCode::NOT_FOUND && !self.dependency_exists(blocked, blocker_id)? {
            return Ok(DependencyChange::AlreadyAbsent);
        }
        Err(mutation_status_error(
            status,
            response.headers(),
            "removing the blocked-by relationship",
        ))
    }

    fn dependency_exists(
        &self,
        blocked: &IssueReference,
        blocker_id: u64,
    ) -> Result<bool, GitHubError> {
        let url = self.dependency_url(blocked)?;
        let blockers: Vec<GitHubBlocker> =
            self.paginate(url, &[("per_page", "50"), ("page", "1")])?;
        Ok(blockers.iter().any(|candidate| candidate.id == blocker_id))
    }

    fn dependency_url(&self, blocked: &IssueReference) -> Result<Url, GitHubError> {
        self.endpoint(&format!(
            "repos/{}/{}/issues/{}/dependencies/blocked_by",
            blocked.repository().owner(),
            blocked.repository().name(),
            blocked.number()
        ))
    }

    fn paginate<T>(
        &self,
        mut url: Url,
        initial_query: &[(&str, &str)],
    ) -> Result<Vec<T>, GitHubError>
    where
        T: DeserializeOwned,
    {
        url.query_pairs_mut()
            .extend_pairs(initial_query.iter().copied());
        let mut results = Vec::new();
        let mut visited = HashSet::new();

        loop {
            if !visited.insert(url.as_str().to_owned()) {
                return Err(GitHubError::PaginationLoop);
            }
            self.require_same_origin(&url)?;

            let response = self
                .client
                .get(url.clone())
                .send()
                .map_err(GitHubError::Request)?;
            let (page, next) = self.decode_page(response)?;
            results.extend(page);
            match next {
                Some(next) => url = next,
                None => return Ok(results),
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
            issues[*index].comments.push(comment.normalize());
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

fn mutation_status_error(
    status: StatusCode,
    headers: &HeaderMap,
    operation: &'static str,
) -> GitHubError {
    if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
        GitHubError::MutationUncertainStatus { operation, status }
    } else {
        api_status_error(status, headers)
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

#[derive(Deserialize)]
struct GitHubIssueLocator {
    id: u64,
    number: u64,
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

#[derive(Serialize)]
struct AddDependency {
    issue_id: u64,
}

#[derive(Debug, Error)]
pub(crate) enum GitHubError {
    #[error("the GitHub token cannot be represented as an HTTP header")]
    InvalidToken,
    #[error("could not build the GitHub HTTP client: {0}")]
    BuildClient(reqwest::Error),
    #[error("GitHub request failed: {0}")]
    Request(reqwest::Error),
    #[error(
        "the outcome of {operation} is uncertain because the GitHub request failed after it may have been sent: {source}; Local replica was not changed"
    )]
    MutationUncertain {
        operation: &'static str,
        source: reqwest::Error,
    },
    #[error(
        "the outcome of {operation} is uncertain after GitHub returned HTTP {status}; Local replica was not changed"
    )]
    MutationUncertainStatus {
        operation: &'static str,
        status: StatusCode,
    },
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
    #[error("GitHub returned an Issue different from the requested Issue")]
    IssueIdentityMismatch,
    #[error("{0} is a Pull Request; native Dependencies require Issues")]
    PullRequestDependency(String),
}
