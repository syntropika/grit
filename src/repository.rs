use thiserror::Error;

pub(crate) struct Repository {
    owner: String,
    name: String,
    full_name: String,
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
