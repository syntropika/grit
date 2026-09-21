use std::{io::Read, time::Duration};

use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
};
use serde::Deserialize;
use url::Url;

use super::{
    AuthError, AuthToken, Host,
    device::{Challenge, DeviceApi, Poll},
};

pub(super) struct GitHubAuth {
    client: Client,
    oauth_base: Url,
    api_base: Url,
}

impl GitHubAuth {
    pub(super) fn new(host: &Host) -> Result<Self, AuthError> {
        Self::with_bases(
            Url::parse(&format!("https://{}/", host.0)).expect("validated host"),
            host.api_url(),
        )
    }

    fn with_bases(oauth_base: Url, api_base: Url) -> Result<Self, AuthError> {
        let client = Client::builder()
            .user_agent(concat!("grit/", env!("CARGO_PKG_VERSION")))
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| AuthError::Network)?;
        Ok(Self {
            client,
            oauth_base,
            api_base,
        })
    }

    fn decode<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, AuthError> {
        let status = response.status();
        if status.as_u16() == 401 {
            return Err(AuthError::Rejected);
        }
        if !status.is_success() {
            return Err(AuthError::Http(status.as_u16()));
        }
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        response
            .take(65_537)
            .read_to_end(&mut bytes)
            .map_err(|_| AuthError::Network)?;
        if bytes.len() > 65_536 {
            return Err(AuthError::Response);
        }
        serde_json::from_slice(&bytes).map_err(|_| AuthError::Response)
    }
}

impl DeviceApi for GitHubAuth {
    fn begin(&self, client_id: &str) -> Result<Challenge, AuthError> {
        let response = self
            .client
            .post(
                self.oauth_base
                    .join("login/device/code")
                    .expect("fixed path"),
            )
            .header("Accept", "application/json")
            .form(&[("client_id", client_id), ("scope", "repo")])
            .send()
            .map_err(|_| AuthError::Network)?;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Start {
            Challenge(Challenge),
            Error(DeviceError),
        }
        match Self::decode(response)? {
            Start::Challenge(challenge) => Ok(challenge),
            Start::Error(error) => Err(error.into_error()),
        }
    }

    fn poll(
        &self,
        client_id: &str,
        device_code: &str,
        timeout: Duration,
    ) -> Result<Poll, AuthError> {
        let response = self
            .client
            .post(
                self.oauth_base
                    .join("login/oauth/access_token")
                    .expect("fixed path"),
            )
            .header("Accept", "application/json")
            .timeout(timeout)
            .form(&[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .map_err(|_| AuthError::Network)?;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Answer {
            Token {
                access_token: String,
                token_type: String,
                expires_in: Option<u64>,
            },
            Error(DeviceError),
        }
        match Self::decode(response)? {
            Answer::Token {
                access_token,
                token_type,
                expires_in,
            } => {
                let access_token = zeroize::Zeroizing::new(access_token);
                if !token_type.eq_ignore_ascii_case("bearer") {
                    return Err(AuthError::Response);
                }
                AuthToken::parse(access_token)?
                    .with_lifetime(expires_in)
                    .map(Poll::Authorized)
            }
            Answer::Error(error) if error.error == "authorization_pending" => Ok(Poll::Pending),
            Answer::Error(error) if error.error == "slow_down" => {
                Ok(Poll::SlowDown(error.interval))
            }
            Answer::Error(error) => Err(error.into_error()),
        }
    }

    fn account(&self, token: &AuthToken) -> Result<String, AuthError> {
        #[derive(Deserialize)]
        struct Account {
            login: String,
        }
        let response = self
            .client
            .get(self.api_base.join("user").expect("fixed path"))
            .header("Accept", "application/vnd.github+json")
            .bearer_auth(token.expose())
            .send()
            .map_err(|_| AuthError::Network)?;
        let account: Account = Self::decode(response)?;
        if account.login.is_empty()
            || account.login.len() > 100
            || !account
                .login
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'[' | b']'))
        {
            return Err(AuthError::Response);
        }
        Ok(account.login)
    }
}

#[derive(Deserialize)]
struct DeviceError {
    error: String,
    interval: Option<u64>,
}
impl DeviceError {
    fn into_error(self) -> AuthError {
        match self.error.as_str() {
            "expired_token" | "token_expired" => AuthError::Expired,
            "access_denied" => AuthError::Denied,
            "device_flow_disabled" => AuthError::DeviceFlowDisabled,
            "incorrect_client_credentials" => AuthError::ClientRejected,
            _ => AuthError::DeviceRejected,
        }
    }
}

#[cfg(test)]
impl GitHubAuth {
    pub(super) fn for_test(base: &str) -> Self {
        let base = Url::parse(base).unwrap();
        Self::with_bases(base.clone(), base).unwrap()
    }
}
