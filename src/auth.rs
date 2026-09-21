use std::{env, process::Command};

use thiserror::Error;

pub(crate) struct AuthToken(String);

impl AuthToken {
    pub(crate) fn discover(hostname: &str) -> Result<Self, AuthError> {
        if let Some(token) = token_from_gh(hostname) {
            return Ok(Self(token));
        }

        if let Some(token) = env::var("GH_TOKEN").ok().and_then(normalize_token) {
            return Ok(Self(token));
        }

        Err(AuthError::Unavailable)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

fn token_from_gh(hostname: &str) -> Option<String> {
    let output = Command::new("gh")
        .args(["auth", "token", "--hostname", hostname])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    normalize_token(String::from_utf8(output.stdout).ok()?)
}

fn normalize_token(value: impl AsRef<str>) -> Option<String> {
    let token = value.as_ref().trim();
    (!token.is_empty()
        && token
            .chars()
            .all(|character| !character.is_whitespace() && !character.is_control()))
    .then(|| token.to_owned())
}

#[derive(Debug, Error)]
pub(crate) enum AuthError {
    #[error("no authenticated gh session or non-empty GH_TOKEN is available")]
    Unavailable,
}
