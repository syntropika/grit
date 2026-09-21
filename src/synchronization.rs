use std::collections::{BTreeMap, BTreeSet};

use crate::{
    dependency_events::{DependencyEvent, RelationshipAction},
    github::{CommentChange, ConditionalPages, DependencyEventWindow, GitHubClient, GitHubError},
    model::{
        BlockerIdentity, BlockerScope, Dependency, DependencyEventCheckpoint, Issue, IssueIdentity,
        Label, LocalReplica, OrdinaryIssueCursor, SyncMetadata, Watermark,
    },
    repository::Repository,
};

pub(crate) struct RepositoryData {
    pub(crate) labels: Vec<Label>,
    pub(crate) issues: Vec<Issue>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) sync: SyncMetadata,
}

pub(crate) fn refresh_repository(
    client: &GitHubClient,
    repository: &Repository,
    previous: Option<&LocalReplica>,
) -> Result<RepositoryData, GitHubError> {
    let labels = client.fetch_labels(repository)?;
    let mut data = if let Some(previous) = previous
        && let Some(cursor) = previous.sync.ordinary_issues.as_ref()
        && let Some(dependency_checkpoint) = previous.sync.dependency_events.as_ref()
        && dependency_checkpoint.latest_event_id.is_some()
    {
        refresh_incremental(client, repository, previous, cursor, dependency_checkpoint)?
    } else {
        refresh_full(client, repository)?
    };
    data.labels = labels;
    Ok(data)
}

fn refresh_full(
    client: &GitHubClient,
    repository: &Repository,
) -> Result<RepositoryData, GitHubError> {
    let pass_started_at = Watermark::now();
    let latest_dependency_event_id = client.fetch_latest_dependency_event_id(repository)?;
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
        labels: Vec::new(),
        issues,
        dependencies: dependencies.into_values().collect(),
        sync: SyncMetadata {
            ordinary_issues: Some(OrdinaryIssueCursor {
                watermark: pass_started_at,
                issues_etag: None,
                comments_etag: None,
            }),
            dependency_events: Some(DependencyEventCheckpoint {
                latest_event_id: latest_dependency_event_id,
            }),
        },
    })
}

fn refresh_incremental(
    client: &GitHubClient,
    repository: &Repository,
    previous: &LocalReplica,
    cursor: &OrdinaryIssueCursor,
    dependency_checkpoint: &DependencyEventCheckpoint,
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

    let (comments_etag, comment_changes) = match comment_delta {
        ConditionalPages::NotModified => (cursor.comments_etag.clone(), Vec::new()),
        ConditionalPages::Modified(page) => (page.safe_etag, page.items),
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

    let event_window =
        client.fetch_dependency_event_window(repository, dependency_checkpoint.latest_event_id)?;
    let (events, next_dependency_checkpoint) = match event_window {
        DependencyEventWindow::Continuous {
            events,
            next_checkpoint,
        } => (events, next_checkpoint),
        DependencyEventWindow::Gap => return refresh_full(client, repository),
    };
    if apply_dependency_events(client, repository, &mut issues, &mut dependencies, events)?
        == EventApplication::NeedsFullReconciliation
    {
        return refresh_full(client, repository);
    }
    let comments_changed = merge_comments(&mut issues, comment_changes);
    issues.sort_by_key(|issue| (issue.number, issue.id));
    if client.fetch_issue_count(repository)? != issues.len() as u64 {
        return refresh_full(client, repository);
    }

    let observed_change = !changed_issue_numbers.is_empty() || comments_changed;
    let watermark = if observed_change {
        cursor.watermark.later(&pass_started_at)
    } else {
        cursor.watermark.clone()
    };
    let watermark_advanced = watermark != cursor.watermark;

    Ok(RepositoryData {
        labels: Vec::new(),
        issues,
        dependencies: dependencies.into_values().collect(),
        sync: SyncMetadata {
            ordinary_issues: Some(OrdinaryIssueCursor {
                watermark,
                issues_etag: (!watermark_advanced).then_some(issue_etag).flatten(),
                comments_etag: (!watermark_advanced).then_some(comments_etag).flatten(),
            }),
            dependency_events: Some(DependencyEventCheckpoint {
                latest_event_id: next_dependency_checkpoint,
            }),
        },
    })
}

#[derive(Eq, PartialEq)]
enum EventApplication {
    Applied,
    NeedsFullReconciliation,
}

fn apply_dependency_events(
    client: &GitHubClient,
    repository: &Repository,
    issues: &mut Vec<Issue>,
    dependencies: &mut BTreeMap<DependencyKey, Dependency>,
    events: Vec<DependencyEvent>,
) -> Result<EventApplication, GitHubError> {
    let mut mutations = Vec::new();
    for event in events.into_iter().rev() {
        match event {
            DependencyEvent::Mutation(mutation) => mutations.push(mutation),
            DependencyEvent::Ignored => {}
            DependencyEvent::ReconcileRequired => {
                return Ok(EventApplication::NeedsFullReconciliation);
            }
        }
    }
    for mutation in mutations {
        let blocked_repository = mutation.blocked.repository.as_str();
        let blocker_repository = mutation.blocker.repository.as_str();
        if !blocked_repository.eq_ignore_ascii_case(repository.full_name()) {
            continue;
        }
        if !ensure_internal_issue(
            client,
            repository,
            issues,
            dependencies,
            mutation.blocked.number,
        )? {
            return Ok(EventApplication::NeedsFullReconciliation);
        }
        let blocker_is_internal = blocker_repository.eq_ignore_ascii_case(repository.full_name());
        if blocker_is_internal
            && !ensure_internal_issue(
                client,
                repository,
                issues,
                dependencies,
                mutation.blocker.number,
            )?
        {
            return Ok(EventApplication::NeedsFullReconciliation);
        }

        let key = DependencyKey {
            blocked_number: mutation.blocked.number,
            blocker_repository: blocker_repository.to_ascii_lowercase(),
            blocker_number: mutation.blocker.number,
        };
        match mutation.action {
            RelationshipAction::Remove => {
                dependencies.remove(&key);
            }
            RelationshipAction::Add => {
                let blocked = issues
                    .iter()
                    .find(|issue| issue.number == mutation.blocked.number)
                    .expect("the blocked Issue was fetched before applying its event");
                let internal_blocker = blocker_is_internal.then(|| {
                    issues
                        .iter()
                        .find(|issue| issue.number == mutation.blocker.number)
                        .expect("the internal blocker was fetched before applying its event")
                });
                dependencies.insert(
                    key,
                    Dependency {
                        blocked: IssueIdentity {
                            repository: repository.full_name().to_owned(),
                            number: blocked.number,
                            id: blocked.id,
                            node_id: blocked.node_id.clone(),
                        },
                        blocker: BlockerIdentity {
                            repository: blocker_repository.to_owned(),
                            number: mutation.blocker.number,
                            state: internal_blocker
                                .map(|issue| issue.state.clone())
                                .unwrap_or(mutation.blocker.state),
                            scope: if blocker_is_internal {
                                BlockerScope::Internal
                            } else {
                                BlockerScope::External
                            },
                            id: internal_blocker.map(|issue| issue.id),
                            node_id: internal_blocker.map(|issue| issue.node_id.clone()),
                        },
                    },
                );
            }
        }
    }
    Ok(EventApplication::Applied)
}

fn ensure_internal_issue(
    client: &GitHubClient,
    repository: &Repository,
    issues: &mut Vec<Issue>,
    dependencies: &mut BTreeMap<DependencyKey, Dependency>,
    number: u64,
) -> Result<bool, GitHubError> {
    if issues.iter().any(|issue| issue.number == number) {
        return Ok(true);
    }
    let Some(issue) = client.fetch_issue(repository, number)? else {
        return Ok(false);
    };
    insert_dependencies(dependencies, client.fetch_dependencies(repository, &issue)?);
    issues.push(issue);
    Ok(true)
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
