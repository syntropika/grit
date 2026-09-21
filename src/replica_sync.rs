use chrono::{SecondsFormat, Utc};
use thiserror::Error;

use crate::{
    github::{GitHubClient, GitHubError},
    model::{LocalReplica, ReplicaError},
    repository::Repository,
    synchronization::refresh_repository,
};

pub(crate) fn fetch(
    client: &GitHubClient,
    repository: &Repository,
) -> Result<LocalReplica, ReplicaSyncError> {
    refresh(client, repository, None)
}

pub(crate) fn refresh(
    client: &GitHubClient,
    repository: &Repository,
    previous: Option<&LocalReplica>,
) -> Result<LocalReplica, ReplicaSyncError> {
    let data = refresh_repository(client, repository, previous)?;
    LocalReplica::build_with_sync(
        repository.full_name().to_owned(),
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        data.sync,
        data.labels,
        data.issues,
        data.dependencies,
    )
    .map_err(Into::into)
}

#[derive(Debug, Error)]
pub(crate) enum ReplicaSyncError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error(transparent)]
    Replica(#[from] ReplicaError),
}
