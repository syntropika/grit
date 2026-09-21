use chrono::{SecondsFormat, Utc};
use thiserror::Error;

use crate::{
    github::{GitHubClient, GitHubError},
    model::{LocalReplica, ReplicaError},
    repository::Repository,
};

pub(crate) fn fetch(
    client: &GitHubClient,
    repository: &Repository,
) -> Result<LocalReplica, ReplicaSyncError> {
    let data = client.fetch_repository(repository)?;
    LocalReplica::build(
        repository.full_name().to_owned(),
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
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
