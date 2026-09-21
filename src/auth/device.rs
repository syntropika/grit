use std::time::{Duration, Instant};

use serde::Deserialize;
use url::Url;

use super::{AuthError, AuthToken, Host};

#[derive(Deserialize)]
pub(super) struct Challenge {
    pub(super) device_code: String,
    pub(super) user_code: String,
    pub(super) verification_uri: String,
    pub(super) expires_in: u64,
    #[serde(default = "default_interval")]
    pub(super) interval: u64,
}

impl Drop for Challenge {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.device_code.zeroize();
    }
}

fn default_interval() -> u64 {
    5
}

pub(super) enum Poll {
    Pending,
    SlowDown(Option<u64>),
    Authorized(AuthToken),
}

pub(super) trait DeviceApi {
    fn begin(&self, client_id: &str) -> Result<Challenge, AuthError>;
    fn poll(
        &self,
        client_id: &str,
        device_code: &str,
        timeout: Duration,
    ) -> Result<Poll, AuthError>;
    fn account(&self, token: &AuthToken) -> Result<String, AuthError>;
}

pub(super) trait Clock {
    fn elapsed(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
}

pub(super) struct SystemClock(Instant);
impl SystemClock {
    pub(super) fn new() -> Self {
        Self(Instant::now())
    }
}
impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub(super) fn authorize(
    api: &impl DeviceApi,
    clock: &mut impl Clock,
    host: &Host,
    client_id: &str,
    display: impl FnOnce(&Challenge),
) -> Result<AuthToken, AuthError> {
    let started = clock.elapsed();
    let challenge = api.begin(client_id)?;
    validate_challenge(&challenge, host)?;
    let deadline = started + Duration::from_secs(challenge.expires_in);
    let mut interval = Duration::from_secs(challenge.interval.max(1));
    display(&challenge);
    loop {
        let remaining = deadline.saturating_sub(clock.elapsed());
        if remaining <= interval {
            return Err(AuthError::Expired);
        }
        clock.sleep(interval);
        let remaining = deadline.saturating_sub(clock.elapsed());
        if remaining.is_zero() {
            return Err(AuthError::Expired);
        }
        match api.poll(
            client_id,
            &challenge.device_code,
            remaining.min(Duration::from_secs(30)),
        )? {
            Poll::Authorized(token) => return Ok(token),
            Poll::Pending => {}
            Poll::SlowDown(recommended) => {
                // RFC 8628 / GitHub require five extra seconds for all subsequent polls.
                interval = interval
                    .saturating_add(Duration::from_secs(5))
                    .max(Duration::from_secs(recommended.unwrap_or(0)));
            }
        }
    }
}

fn validate_challenge(challenge: &Challenge, host: &Host) -> Result<(), AuthError> {
    let uri = Url::parse(&challenge.verification_uri).map_err(|_| AuthError::Response)?;
    if uri.scheme() != "https"
        || uri.host_str() != Some(&host.0)
        || uri.port_or_known_default() != Some(443)
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
        || uri.path() != "/login/device"
        || challenge.expires_in == 0
        || challenge.expires_in > 3600
        || challenge.interval > 3600
        || challenge.device_code.is_empty()
        || challenge.device_code.len() > 1024
        || challenge
            .device_code
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        || challenge.user_code.is_empty()
        || challenge.user_code.len() > 64
        || !challenge
            .user_code
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(AuthError::Response);
    }
    Ok(())
}
