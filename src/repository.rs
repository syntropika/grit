use thiserror::Error;

pub(crate) struct Repository {
    owner: String,
    name: String,
    full_name: String,
}

pub(crate) struct IssueReference {
    repository: Repository,
    number: u64,
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
