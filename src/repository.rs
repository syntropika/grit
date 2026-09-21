use std::str::FromStr;

use thiserror::Error;

use crate::model::TemporaryIssueId;

#[derive(Clone)]
pub(crate) struct Repository {
    owner: String,
    name: String,
    full_name: String,
}

#[derive(Clone)]
pub(crate) struct IssueReference {
    repository: Repository,
    number: u64,
}

pub(crate) enum PendingIssueReference {
    GitHub(IssueReference),
    Draft {
        repository: Repository,
        temporary_id: TemporaryIssueId,
    },
}

impl PendingIssueReference {
    pub(crate) fn parse(value: &str) -> Result<Self, PendingIssueReferenceError> {
        let Some((repository, target)) = value.split_once('#') else {
            return Err(PendingIssueReferenceError);
        };
        if target.contains('#') {
            return Err(PendingIssueReferenceError);
        }
        let repository = Repository::parse(repository).map_err(|_| PendingIssueReferenceError)?;
        if let Some(temporary_id) = target.strip_prefix("draft:") {
            return TemporaryIssueId::from_str(temporary_id)
                .map(|temporary_id| Self::Draft {
                    repository,
                    temporary_id,
                })
                .map_err(|_| PendingIssueReferenceError);
        }
        let number = target
            .parse::<u64>()
            .ok()
            .filter(|number| *number > 0)
            .ok_or(PendingIssueReferenceError)?;
        Ok(Self::GitHub(IssueReference { repository, number }))
    }

    pub(crate) fn repository(&self) -> &Repository {
        match self {
            Self::GitHub(reference) => reference.repository(),
            Self::Draft { repository, .. } => repository,
        }
    }

    pub(crate) fn temporary_id(&self) -> Option<TemporaryIssueId> {
        match self {
            Self::GitHub(_) => None,
            Self::Draft { temporary_id, .. } => Some(*temporary_id),
        }
    }

    pub(crate) fn local_number(&self) -> u64 {
        match self {
            Self::GitHub(reference) => reference.number(),
            Self::Draft { temporary_id, .. } => temporary_id.synthetic_number(),
        }
    }

    pub(crate) fn as_github(&self) -> Option<&IssueReference> {
        match self {
            Self::GitHub(reference) => Some(reference),
            Self::Draft { .. } => None,
        }
    }

    pub(crate) fn github_reference(&self, number: u64) -> IssueReference {
        IssueReference {
            repository: self.repository().clone(),
            number,
        }
    }

    pub(crate) fn stable_key(&self) -> String {
        match self {
            Self::GitHub(reference) => reference.stable_key(),
            Self::Draft {
                repository,
                temporary_id,
            } => temporary_id.stable_node_key(repository.full_name()),
        }
    }
}

impl IssueReference {
    pub(crate) fn parse(value: &str) -> Result<Self, IssueReferenceError> {
        let Some((repository, number)) = value.split_once('#') else {
            return Err(IssueReferenceError);
        };
        if number.contains('#') {
            return Err(IssueReferenceError);
        }
        let repository = Repository::parse(repository).map_err(|_| IssueReferenceError)?;
        let number = number
            .parse::<u64>()
            .ok()
            .filter(|number| *number > 0)
            .ok_or(IssueReferenceError)?;
        Ok(Self { repository, number })
    }

    pub(crate) fn repository(&self) -> &Repository {
        &self.repository
    }

    pub(crate) fn number(&self) -> u64 {
        self.number
    }

    pub(crate) fn stable_key(&self) -> String {
        format!("{}#{}", self.repository.full_name(), self.number)
    }
}

impl Repository {
    pub(crate) fn parse(value: &str) -> Result<Self, RepositoryError> {
        let Some((owner, name)) = value.split_once('/') else {
            return Err(RepositoryError);
        };
        if name.contains('/') || !valid_part(owner) || !valid_part(name) {
            return Err(RepositoryError);
        }
        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
            full_name: format!("{owner}/{name}"),
        })
    }

    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn full_name(&self) -> &str {
        &self.full_name
    }
}

fn valid_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[derive(Debug, Error)]
#[error("repository must use a safe OWNER/REPO form")]
pub(crate) struct RepositoryError;

#[derive(Debug, Error)]
#[error("Issue reference must use a safe OWNER/REPO#NUMBER form with NUMBER greater than zero")]
pub(crate) struct IssueReferenceError;

#[derive(Debug, Error)]
#[error("Issue reference must use OWNER/REPO#NUMBER or OWNER/REPO#draft:TEMPORARY_ID form")]
pub(crate) struct PendingIssueReferenceError;
