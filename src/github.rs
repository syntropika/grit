use std::collections::HashSet;

use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{
        ACCEPT, AUTHORIZATION, ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH, LINK, USER_AGENT,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use thiserror::Error;
use url::Url;

use crate::{
    auth::AuthToken,
    dependency_events::{DependencyEvent, RawEvent, RawIssueReference},
    model::{
        Actor, BlockerIdentity, BlockerScope, Comment, Dependency, EntityTag, Issue, IssueIdentity,
        Label,
    },
    repository::{IssueReference, Repository},
};

const API_VERSION: &str = "2026-03-10";

pub(crate) struct GitHubClient {
    client: Client,
    base_url: Url,
}

pub(crate) enum LabelCreation {
    Created,
    AlreadyPresent,
}

#[derive(Serialize)]
pub(crate) struct CreateLabelRequest<'a> {
    name: &'a str,
    color: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct AddIssueLabelsRequest<'a> {
    labels: &'a [String],
}

impl<'a> CreateLabelRequest<'a> {
    pub(crate) fn new(name: &'a str, color: &'a str, description: &'a str) -> Self {
        Self {
            name,
            color,
            description,
        }
    }
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

#[derive(Clone, Deserialize)]
pub(crate) struct RepositoryMetadata {
    pub(crate) full_name: String,
    pub(crate) html_url: String,
    pub(crate) visibility: String,
    pub(crate) private: bool,
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

    pub(crate) fn fetch_all_issues(
        &self,
        repository: &Repository,
    ) -> Result<Vec<Issue>, GitHubError> {
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

        let mut issues: Vec<_> = raw_issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(GitHubIssue::normalize)
            .collect();
        issues.sort_by_key(|issue| (issue.number, issue.id));
        Ok(issues)
    }

    pub(crate) fn fetch_all_comments(
        &self,
        repository: &Repository,
    ) -> Result<Vec<CommentChange>, GitHubError> {
        let owner = repository.owner();
        let repo = repository.name();
        let comments_url = self.endpoint(&format!("repos/{owner}/{repo}/issues/comments"))?;
        let raw_comments: Vec<GitHubComment> =
            self.paginate(comments_url, &[("per_page", "100")])?;
        Ok(raw_comments
            .into_iter()
            .filter_map(GitHubComment::normalize_change)
            .collect())
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

    pub(crate) fn fetch_issue_for_update(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<Issue, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/{number}",
            repository.owner(),
            repository.name()
        ))?;
        let response = self.client.get(url).send().map_err(GitHubError::Request)?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }
        let issue: GitHubIssue = response.json().map_err(GitHubError::Decode)?;
        if issue.number != number {
            return Err(GitHubError::IssueIdentityMismatch);
        }
        if issue.pull_request.is_some() {
            return Err(GitHubError::PullRequestPriority(format!(
                "{}#{number}",
                repository.full_name()
            )));
        }
        Ok(issue.normalize())
    }

    pub(crate) fn add_issue_label(
        &self,
        repository: &Repository,
        number: u64,
        label: &str,
    ) -> Result<(), GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/{number}/labels",
            repository.owner(),
            repository.name()
        ))?;
        let labels = [label.to_owned()];
        let response = self
            .client
            .post(url)
            .json(&AddIssueLabelsRequest { labels: &labels })
            .send()
            .map_err(|source| GitHubError::MutationUncertain {
                operation: "adding the requested Priority label",
                source,
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        Err(mutation_status_error(
            status,
            response.headers(),
            "adding the requested Priority label",
        ))
    }

    pub(crate) fn remove_issue_label(
        &self,
        repository: &Repository,
        number: u64,
        label: &str,
    ) -> Result<(), GitHubError> {
        let mut url = self.endpoint(&format!(
            "repos/{}/{}/issues/{number}/labels",
            repository.owner(),
            repository.name()
        ))?;
        url.path_segments_mut()
            .map_err(|_| GitHubError::InvalidLabelUrl)?
            .push(label);
        let response =
            self.client
                .delete(url)
                .send()
                .map_err(|source| GitHubError::MutationUncertain {
                    operation: "removing an obsolete Priority label",
                    source,
                })?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(mutation_status_error(
            status,
            response.headers(),
            "removing an obsolete Priority label",
        ))
    }

    pub(crate) fn fetch_repository_metadata(
        &self,
        repository: &Repository,
    ) -> Result<RepositoryMetadata, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}",
            repository.owner(),
            repository.name()
        ))?;
        let response = self.client.get(url).send().map_err(GitHubError::Request)?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }
        response.json().map_err(GitHubError::DecodeMetadata)
    }

    pub(crate) fn fetch_labels(&self, repository: &Repository) -> Result<Vec<Label>, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/labels",
            repository.owner(),
            repository.name()
        ))?;
        let labels: Vec<GitHubLabel> = self.paginate(url, &[("per_page", "100")])?;
        let mut labels: Vec<_> = labels.into_iter().map(GitHubLabel::normalize).collect();
        labels.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(labels)
    }

    pub(crate) fn create_label(
        &self,
        repository: &Repository,
        request: &CreateLabelRequest<'_>,
    ) -> Result<LabelCreation, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/labels",
            repository.owner(),
            repository.name()
        ))?;
        let response = self
            .client
            .post(url)
            .json(request)
            .send()
            .map_err(GitHubError::Request)?;
        let status = response.status();
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            let headers = response.headers().clone();
            let body = response.text().map_err(GitHubError::Decode)?;
            let already_exists =
                serde_json::from_str::<GitHubApiErrorResponse>(&body).is_ok_and(|error| {
                    error.errors.iter().any(|detail| {
                        detail.code == "already_exists" && detail.field.as_deref() == Some("name")
                    })
                });
            if already_exists {
                return Ok(LabelCreation::AlreadyPresent);
            }
            return Err(api_status_error(status, &headers));
        }
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }
        let _: GitHubLabel = response.json().map_err(GitHubError::Decode)?;
        Ok(LabelCreation::Created)
    }

    pub(crate) fn fetch_issue_delta(
        &self,
        repository: &Repository,
        since: &str,
        etag: Option<&EntityTag>,
    ) -> Result<ConditionalPages<Issue>, GitHubError> {
        let owner = repository.owner();
        let repo = repository.name();
        let issue_url = self.endpoint(&format!("repos/{owner}/{repo}/issues"))?;
        self.paginate_conditional::<GitHubIssue>(
            issue_url,
            &[
                ("state", "all"),
                ("sort", "created"),
                ("direction", "asc"),
                ("since", since),
                ("per_page", "100"),
            ],
            etag.map(EntityTag::as_str),
        )
        .map(|pages| {
            pages.filter_map(|issue| issue.pull_request.is_none().then(|| issue.normalize()))
        })
    }

    pub(crate) fn fetch_comment_delta(
        &self,
        repository: &Repository,
        since: &str,
        etag: Option<&EntityTag>,
    ) -> Result<ConditionalPages<CommentChange>, GitHubError> {
        let owner = repository.owner();
        let repo = repository.name();
        let comments_url = self.endpoint(&format!("repos/{owner}/{repo}/issues/comments"))?;
        self.paginate_conditional::<GitHubComment>(
            comments_url,
            &[
                ("sort", "created"),
                ("direction", "asc"),
                ("since", since),
                ("per_page", "100"),
            ],
            etag.map(EntityTag::as_str),
        )
        .map(|pages| pages.filter_map(GitHubComment::normalize_change))
    }

    pub(crate) fn fetch_dependencies(
        &self,
        repository: &Repository,
        issue: &Issue,
    ) -> Result<Vec<Dependency>, GitHubError> {
        let dependency_url = self.endpoint(&format!(
            "repos/{}/{}/issues/{}/dependencies/blocked_by",
            repository.owner(),
            repository.name(),
            issue.number
        ))?;
        let blockers: Vec<GitHubBlocker> = self.paginate(dependency_url, &[("per_page", "100")])?;
        blockers
            .into_iter()
            .map(|blocker| normalize_dependency(repository.full_name(), issue, blocker))
            .collect()
    }

    pub(crate) fn fetch_issue(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<Option<Issue>, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/{number}",
            repository.owner(),
            repository.name()
        ))?;
        let response = self.client.get(url).send().map_err(GitHubError::Request)?;
        if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) {
            return Ok(None);
        }
        let status = response.status();
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }
        let issue: GitHubIssue = response.json().map_err(GitHubError::Decode)?;
        if issue.pull_request.is_some() {
            return Ok(None);
        }
        let mut issue = issue.normalize();
        let comments_url = self.endpoint(&format!(
            "repos/{}/{}/issues/{number}/comments",
            repository.owner(),
            repository.name()
        ))?;
        let comments: Vec<GitHubComment> = self.paginate(comments_url, &[("per_page", "100")])?;
        issue.comments = comments.into_iter().map(GitHubComment::normalize).collect();
        issue.comments.sort_by_key(|comment| comment.id);
        Ok(Some(issue))
    }

    pub(crate) fn fetch_dependency_event_window(
        &self,
        repository: &Repository,
        checkpoint: Option<u64>,
    ) -> Result<DependencyEventWindow, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/events",
            repository.owner(),
            repository.name()
        ))?;
        let mut event_ids = HashSet::new();
        let mut events = Vec::new();
        let mut checkpoint_found = checkpoint.is_none();
        let mut next_checkpoint = checkpoint;
        self.walk_pages(
            url,
            &[("per_page", "100")],
            |page: Vec<GitHubIssueEvent>| {
                for event in page {
                    if Some(event.id) == checkpoint {
                        checkpoint_found = true;
                        return Ok(PageFlow::Stop);
                    }
                    if event_ids.insert(event.id) {
                        if next_checkpoint == checkpoint {
                            next_checkpoint = Some(event.id);
                        }
                        events.push(event.normalize(repository.full_name())?);
                    }
                }
                Ok(PageFlow::Continue)
            },
        )?;

        if checkpoint_found {
            Ok(DependencyEventWindow::Continuous {
                events,
                next_checkpoint,
            })
        } else {
            Ok(DependencyEventWindow::Gap)
        }
    }

    pub(crate) fn fetch_latest_dependency_event_id(
        &self,
        repository: &Repository,
    ) -> Result<Option<u64>, GitHubError> {
        let url = self.endpoint(&format!(
            "repos/{}/{}/issues/events",
            repository.owner(),
            repository.name()
        ))?;
        let mut latest = None;
        self.walk_pages(
            url,
            &[("per_page", "100")],
            |events: Vec<GitHubEventIdentity>| {
                latest = events.first().map(|event| event.id);
                Ok(PageFlow::Stop)
            },
        )?;
        Ok(latest)
    }

    pub(crate) fn fetch_issue_count(&self, repository: &Repository) -> Result<u64, GitHubError> {
        let response = self
            .client
            .post(self.graphql_endpoint())
            .json(&json!({
                "query": "query IssueInventoryCount($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { issues(first: 1) { totalCount } } }",
                "variables": {
                    "owner": repository.owner(),
                    "name": repository.name()
                }
            }))
            .send()
            .map_err(GitHubError::Request)?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_status_error(status, response.headers()));
        }
        let payload: GitHubIssueCountResponse = response.json().map_err(GitHubError::Decode)?;
        if !payload.errors.is_empty() {
            return Err(GitHubError::GraphQl);
        }
        payload
            .data
            .and_then(|data| data.repository)
            .map(|repository| repository.issues.total_count)
            .ok_or(GitHubError::GraphQl)
    }

    fn paginate<T>(&self, url: Url, initial_query: &[(&str, &str)]) -> Result<Vec<T>, GitHubError>
    where
        T: DeserializeOwned,
    {
        let mut results = Vec::new();
        self.walk_pages(url, initial_query, |page| {
            results.extend(page);
            Ok(PageFlow::Continue)
        })?;
        Ok(results)
    }

    fn walk_pages<T>(
        &self,
        mut url: Url,
        initial_query: &[(&str, &str)],
        mut visit: impl FnMut(Vec<T>) -> Result<PageFlow, GitHubError>,
    ) -> Result<(), GitHubError>
    where
        T: DeserializeOwned,
    {
        url.query_pairs_mut()
            .extend_pairs(initial_query.iter().copied());
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
            if visit(page)? == PageFlow::Stop {
                return Ok(());
            }
            match next {
                Some(next) => url = next,
                None => return Ok(()),
            }
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
                    .and_then(EntityTag::parse);
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

    fn graphql_endpoint(&self) -> Url {
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        let path = match base_path.strip_suffix("/api/v3") {
            Some(prefix) => format!("{prefix}/api/graphql"),
            None if base_path.is_empty() => "/graphql".to_owned(),
            None => format!("{base_path}/graphql"),
        };
        endpoint.set_path(&path);
        endpoint
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

pub(crate) enum ConditionalPages<T> {
    NotModified,
    Modified(CompletePages<T>),
}

#[derive(Eq, PartialEq)]
enum PageFlow {
    Continue,
    Stop,
}

impl<T> ConditionalPages<T> {
    fn filter_map<U>(self, map: impl FnMut(T) -> Option<U>) -> ConditionalPages<U> {
        match self {
            Self::NotModified => ConditionalPages::NotModified,
            Self::Modified(page) => ConditionalPages::Modified(CompletePages {
                items: page.items.into_iter().filter_map(map).collect(),
                safe_etag: page.safe_etag,
            }),
        }
    }
}

pub(crate) struct CompletePages<T> {
    pub(crate) items: Vec<T>,
    pub(crate) safe_etag: Option<EntityTag>,
}

pub(crate) struct CommentChange {
    pub(crate) issue_number: u64,
    pub(crate) comment: Comment,
}

pub(crate) enum DependencyEventWindow {
    Continuous {
        events: Vec<DependencyEvent>,
        next_checkpoint: Option<u64>,
    },
    Gap,
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

#[derive(Deserialize)]
struct GitHubApiErrorResponse {
    #[serde(default)]
    errors: Vec<GitHubValidationError>,
}

#[derive(Deserialize)]
struct GitHubValidationError {
    code: String,
    field: Option<String>,
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
    fn normalize_change(self) -> Option<CommentChange> {
        let issue_number = issue_number_from_url(&self.issue_url)?;
        Some(CommentChange {
            issue_number,
            comment: self.normalize(),
        })
    }

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

#[derive(Deserialize)]
struct GitHubIssueEvent {
    id: u64,
    event: String,
    created_at: String,
    issue: Option<GitHubEventIssueReference>,
    blocked_by: Option<GitHubEventIssueReference>,
    blocking: Option<GitHubEventIssueReference>,
}

#[derive(Deserialize)]
struct GitHubEventIdentity {
    id: u64,
}

impl GitHubIssueEvent {
    fn normalize(self, repository: &str) -> Result<DependencyEvent, GitHubError> {
        Ok(DependencyEvent::classify(RawEvent {
            kind: self.event,
            created_at: self.created_at,
            issue: self
                .issue
                .map(|issue| issue.normalize(Some(repository)))
                .transpose()?,
            blocked_by: self
                .blocked_by
                .map(|issue| issue.normalize(None))
                .transpose()?,
            blocking: self
                .blocking
                .map(|issue| issue.normalize(None))
                .transpose()?,
        }))
    }
}

#[derive(Deserialize)]
struct GitHubEventIssueReference {
    number: u64,
    #[serde(default)]
    state: String,
    repository: Option<GitHubEventRepository>,
    repository_url: Option<String>,
}

impl GitHubEventIssueReference {
    fn normalize(self, default_repository: Option<&str>) -> Result<RawIssueReference, GitHubError> {
        let repository = match (self.repository, self.repository_url) {
            (Some(repository), _) => Some(repository.full_name),
            (None, Some(url)) => Some(repository_from_api_url(&url)?),
            (None, None) => default_repository.map(str::to_owned),
        };
        Ok(RawIssueReference {
            repository,
            number: self.number,
            state: self.state,
        })
    }
}

#[derive(Deserialize)]
struct GitHubEventRepository {
    full_name: String,
}

#[derive(Deserialize)]
struct GitHubIssueCountResponse {
    data: Option<GitHubIssueCountData>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct GitHubIssueCountData {
    repository: Option<GitHubIssueCountRepository>,
}

#[derive(Deserialize)]
struct GitHubIssueCountRepository {
    issues: GitHubIssueCountConnection,
}

#[derive(Deserialize)]
struct GitHubIssueCountConnection {
    #[serde(rename = "totalCount")]
    total_count: u64,
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
    #[error("GitHub returned invalid JSON: {0}")]
    Decode(reqwest::Error),
    #[error("GitHub returned invalid Repository metadata: {0}")]
    DecodeMetadata(reqwest::Error),
    #[error("the stored GitHub ETag cannot be represented as an HTTP header")]
    InvalidEtag,
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
    #[error("GitHub GraphQL did not return the Repository Issue count")]
    GraphQl,
    #[error("GitHub returned an Issue different from the requested Issue")]
    IssueIdentityMismatch,
    #[error("{0} is a Pull Request; native Dependencies require Issues")]
    PullRequestDependency(String),
    #[error("could not construct a safe Issue-label URL")]
    InvalidLabelUrl,
    #[error("{0} is a Pull Request; Declared priority updates require an Issue")]
    PullRequestPriority(String),
}
