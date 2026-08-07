use std::collections::{BTreeMap, BTreeSet};

use crate::{
    github::{CommentChange, ConditionalPages, GitHubClient, GitHubError},
    model::{Dependency, Issue, LocalReplica, OrdinaryIssueCursor, SyncMetadata, Watermark},
    repository::Repository,
};

pub(crate) struct RepositoryData {
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) sync: SyncMetadata,
}

pub(crate) fn refresh_repository(
    client: &GitHubClient,
    repository: &Repository,
    previous: Option<&LocalReplica>,
) -> Result<RepositoryData, GitHubError> {
    if let Some(previous) = previous
        && let Some(cursor) = previous.sync.ordinary_issues.as_ref()
    {
        return refresh_incremental(client, repository, previous, cursor);
    }
    refresh_full(client, repository)
}

fn refresh_full(
    client: &GitHubClient,
    repository: &Repository,
) -> Result<RepositoryData, GitHubError> {
    let pass_started_at = Watermark::now();
    let mut issues = client.fetch_all_issues(repository)?;
    merge_comments(&mut issues, client.fetch_all_comments(repository)?);

    let mut dependencies = BTreeMap::new();
    for issue in &issues {
        insert_dependencies(
            &mut dependencies,
            client.fetch_dependencies(repository, issue)?,
        );
    }

    Ok(RepositoryData {
        issues,
        dependencies: dependencies.into_values().collect(),
        sync: SyncMetadata {
            ordinary_issues: Some(OrdinaryIssueCursor {
                watermark: pass_started_at,
                issues_etag: None,
                comments_etag: None,
            }),
        },
    })
}

fn refresh_incremental(
    client: &GitHubClient,
    repository: &Repository,
    previous: &LocalReplica,
    cursor: &OrdinaryIssueCursor,
) -> Result<RepositoryData, GitHubError> {
    let pass_started_at = Watermark::now();
    let since = cursor.watermark.overlapped_since();
    let issue_delta = client.fetch_issue_delta(repository, &since, cursor.issues_etag.as_ref())?;
    let comment_delta =
        client.fetch_comment_delta(repository, &since, cursor.comments_etag.as_ref())?;

    let mut issues = previous.issues.clone();
    let mut changed_issue_numbers = BTreeSet::new();
    let issue_etag = match issue_delta {
        ConditionalPages::NotModified => cursor.issues_etag.clone(),
        ConditionalPages::Modified(page) => {
            for mut incoming in page.items {
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

    let (comments_etag, comments_changed) = match comment_delta {
        ConditionalPages::NotModified => (cursor.comments_etag.clone(), false),
        ConditionalPages::Modified(page) => {
            let changed = merge_comments(&mut issues, page.items);
            (page.safe_etag, changed)
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
        insert_dependencies(
            &mut dependencies,
            client.fetch_dependencies(repository, issue)?,
        );
    }

    let observed_change = !changed_issue_numbers.is_empty() || comments_changed;
    let watermark = if observed_change {
        cursor.watermark.later(&pass_started_at)
    } else {
        cursor.watermark.clone()
    };
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

fn merge_comments(issues: &mut [Issue], comments: Vec<CommentChange>) -> bool {
    let by_number: BTreeMap<u64, usize> = issues
        .iter()
        .enumerate()
        .map(|(index, issue)| (issue.number, index))
        .collect();
    let mut changed = false;

    for change in comments {
        let Some(index) = by_number.get(&change.issue_number) else {
            continue;
        };
        match issues[*index]
            .comments
            .iter()
            .position(|existing| existing.id == change.comment.id)
        {
            Some(comment_index) if issues[*index].comments[comment_index] != change.comment => {
                issues[*index].comments[comment_index] = change.comment;
                changed = true;
            }
            Some(_) => {}
            None => {
                issues[*index].comments.push(change.comment);
                changed = true;
            }
        }
    }
    for issue in issues {
        issue.comments.sort_by_key(|comment| comment.id);
    }
    changed
}

fn insert_dependencies(
    target: &mut BTreeMap<DependencyKey, Dependency>,
    dependencies: Vec<Dependency>,
) {
    for dependency in dependencies {
        target
            .entry(DependencyKey::from(&dependency))
            .or_insert(dependency);
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
