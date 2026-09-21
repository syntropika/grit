use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

use mockito::{Matcher, Server};
use serde_json::json;

use super::{
    device::{Challenge, Clock, Poll},
    *,
};

#[derive(Default)]
struct MemoryStore {
    values: RefCell<BTreeMap<String, String>>,
    writes: Cell<usize>,
    unavailable: bool,
}
impl CredentialStore for MemoryStore {
    fn get(&self, host: &Host) -> Result<Option<AuthToken>, AuthError> {
        if self.unavailable {
            return Err(AuthError::SecureStore);
        }
        self.values
            .borrow()
            .get(&host.0)
            .map(AuthToken::parse)
            .transpose()
    }
    fn set(&self, host: &Host, token: &AuthToken) -> Result<(), AuthError> {
        if self.unavailable {
            return Err(AuthError::SecureStore);
        }
        self.writes.set(self.writes.get() + 1);
        self.values
            .borrow_mut()
            .insert(host.0.clone(), token.expose().to_owned());
        Ok(())
    }
    fn delete(&self, host: &Host) -> Result<bool, AuthError> {
        Ok(self.values.borrow_mut().remove(&host.0).is_some())
    }
}

#[derive(Default)]
struct FakeClock {
    elapsed: Duration,
    sleeps: Vec<Duration>,
}
impl Clock for FakeClock {
    fn elapsed(&self) -> Duration {
        self.elapsed
    }
    fn sleep(&mut self, duration: Duration) {
        self.elapsed += duration;
        self.sleeps.push(duration);
    }
}

struct FakeApi {
    replies: RefCell<VecDeque<Result<Poll, AuthError>>>,
    lifetime: u64,
    verified: bool,
}
impl DeviceApi for FakeApi {
    fn begin(&self, _: &str) -> Result<Challenge, AuthError> {
        Ok(challenge(self.lifetime))
    }
    fn poll(&self, _: &str, code: &str, _: Duration) -> Result<Poll, AuthError> {
        assert_eq!(code, "secret-device-code");
        self.replies
            .borrow_mut()
            .pop_front()
            .expect("unexpected extra poll")
    }
    fn account(&self, _: &AuthToken) -> Result<String, AuthError> {
        if self.verified {
            Ok("octocat".to_owned())
        } else {
            Err(AuthError::Rejected)
        }
    }
}
fn fake_api(replies: Vec<Result<Poll, AuthError>>) -> FakeApi {
    FakeApi {
        replies: RefCell::new(replies.into()),
        lifetime: 60,
        verified: true,
    }
}
fn challenge(lifetime: u64) -> Challenge {
    Challenge {
        device_code: "secret-device-code".to_owned(),
        user_code: "CODE-ABCD".to_owned(),
        verification_uri: "https://github.com/login/device".to_owned(),
        expires_in: lifetime,
        interval: 5,
    }
}
fn token(value: &str) -> AuthToken {
    AuthToken::parse(value).unwrap()
}
fn github() -> Host {
    Host::parse("github.com").unwrap()
}

#[test]
fn official_oauth_app_is_the_default_only_for_github_com() {
    for hostname in ["github.com", "GitHub.COM."] {
        let host = Host::parse(hostname).unwrap();
        assert_eq!(
            configured_client_id(&host, None, None).unwrap(),
            "Ov23lie2fyBwnR4nRGgA"
        );
    }
    for hostname in [
        "github.example.com",
        "api.github.com",
        "github.com.example.com",
        "example.github.com",
    ] {
        let host = Host::parse(hostname).unwrap();
        assert!(matches!(
            configured_client_id(&host, None, None),
            Err(AuthError::MissingClientId)
        ));
    }
}

#[test]
fn oauth_client_id_overrides_apply_in_order_on_public_and_enterprise_hosts() {
    for hostname in ["github.com", "github.example.com"] {
        let host = Host::parse(hostname).unwrap();
        assert_eq!(
            configured_client_id(&host, None, Some("environment-app".to_owned())).unwrap(),
            "environment-app"
        );
        assert_eq!(
            configured_client_id(
                &host,
                Some("flag-app".to_owned()),
                Some("environment-app".to_owned()),
            )
            .unwrap(),
            "flag-app"
        );
        assert_eq!(
            configured_client_id(&host, Some("flag-app".to_owned()), Some(String::new())).unwrap(),
            "flag-app"
        );
    }
}

#[test]
fn invalid_explicit_oauth_client_ids_do_not_fall_back_to_another_app() {
    for invalid in [
        "",
        "   ",
        "app with spaces",
        "app/invalid",
        &"a".repeat(257),
    ] {
        for hostname in ["github.com", "github.example.com"] {
            let host = Host::parse(hostname).unwrap();
            assert!(matches!(
                configured_client_id(
                    &host,
                    Some(invalid.to_owned()),
                    Some("environment-app".to_owned()),
                ),
                Err(AuthError::InvalidClientId)
            ));
            assert!(matches!(
                configured_client_id(&host, None, Some(invalid.to_owned())),
                Err(AuthError::InvalidClientId)
            ));
        }
    }
}

#[test]
fn explicit_environment_token_wins_without_opening_the_store() {
    let result = discover_with(Some(" environment-token \n".to_owned()), || {
        panic!("must not access keychain")
    })
    .unwrap();
    assert_eq!(result.source, CredentialSource::Environment);
    assert_eq!(result.token.expose(), "environment-token");
    assert!(matches!(
        discover_with(Some("token with spaces".to_owned()), || panic!()),
        Err(AuthError::InvalidToken)
    ));
}

#[test]
fn saved_grit_token_is_used_when_no_environment_token_is_set() {
    let result = discover_with(None, || Ok(Some(token("saved-token")))).unwrap();
    assert_eq!(result.source, CredentialSource::Grit);
    assert!(matches!(
        discover_with(None, || Err(AuthError::SecureStore)),
        Err(AuthError::SecureStore)
    ));
    assert!(matches!(
        discover_with(Some(" \n".to_owned()), || Ok(None)),
        Err(AuthError::Unavailable)
    ));
}

#[test]
fn secure_credentials_are_scoped_to_normalized_hosts_and_logout_preserves_other_hosts() {
    let store = MemoryStore::default();
    let public = Host::parse("GitHub.COM.").unwrap();
    let enterprise = Host::parse("github.example.com").unwrap();
    store.set(&public, &token("public-token")).unwrap();
    store.set(&enterprise, &token("enterprise-token")).unwrap();
    assert!(store.delete(&public).unwrap());
    assert!(store.get(&public).unwrap().is_none());
    assert_eq!(
        store.get(&enterprise).unwrap().unwrap().expose(),
        "enterprise-token"
    );
    assert!(!store.delete(&public).unwrap());
}

#[test]
fn host_and_canonical_api_validation_prevent_saved_token_redirects() {
    for invalid in [
        "",
        "https://github.com",
        "github.com/path",
        "user@github.com",
        "github.com:443",
        "github.com?x",
        "a..b",
        "-bad.example",
        "github.com\n.attacker",
    ] {
        assert!(Host::parse(invalid).is_err(), "accepted {invalid}");
    }
    let public = github();
    assert!(public.matches_api(&Url::parse("https://api.github.com/").unwrap()));
    for invalid in [
        "http://api.github.com/",
        "https://api.github.com:444/",
        "https://attacker.example/",
        "https://api.github.com/proxy/",
        "https://user@api.github.com/",
        "https://api.github.com/?secret=1",
    ] {
        assert!(
            !public.matches_api(&Url::parse(invalid).unwrap()),
            "accepted {invalid}"
        );
    }
    let enterprise = Host::parse("github.example").unwrap();
    assert_eq!(
        enterprise.api_url().as_str(),
        "https://github.example/api/v3/"
    );
    assert!(enterprise.matches_api(&enterprise.api_url()));
}

#[test]
fn device_polling_waits_before_first_request_and_honors_slow_down() {
    let api = fake_api(vec![
        Ok(Poll::Pending),
        Ok(Poll::SlowDown(Some(12))),
        Ok(Poll::Pending),
        Ok(Poll::Authorized(token("authorized-token"))),
    ]);
    let mut clock = FakeClock::default();
    let mut displayed = false;
    let result = device::authorize(&api, &mut clock, &github(), "our-client-id", |challenge| {
        assert_eq!(challenge.user_code, "CODE-ABCD");
        displayed = true;
    })
    .unwrap();
    assert!(displayed);
    assert_eq!(result.expose(), "authorized-token");
    assert_eq!(clock.sleeps, [5, 5, 12, 12].map(Duration::from_secs));
}

#[test]
fn repeated_slow_down_always_adds_five_seconds_even_if_response_interval_is_smaller() {
    let mut api = fake_api(vec![
        Ok(Poll::SlowDown(None)),
        Ok(Poll::SlowDown(Some(1))),
        Ok(Poll::Authorized(token("authorized-token"))),
    ]);
    api.lifetime = 120;
    let mut clock = FakeClock::default();
    device::authorize(&api, &mut clock, &github(), "our-client-id", |_| {}).unwrap();
    assert_eq!(clock.sleeps, [5, 10, 15].map(Duration::from_secs));
}

#[test]
fn device_polling_stops_at_expiry_and_denial_without_extra_requests() {
    let mut api = fake_api(vec![Ok(Poll::Pending)]);
    api.lifetime = 10;
    let mut clock = FakeClock::default();
    assert!(matches!(
        device::authorize(&api, &mut clock, &github(), "our-client-id", |_| {}),
        Err(AuthError::Expired)
    ));
    assert_eq!(clock.sleeps, [Duration::from_secs(5)]);
    let api = fake_api(vec![Err(AuthError::Denied)]);
    assert!(matches!(
        device::authorize(
            &api,
            &mut FakeClock::default(),
            &github(),
            "our-client-id",
            |_| {}
        ),
        Err(AuthError::Denied)
    ));
}

#[test]
fn account_verification_precedes_storage_and_rejected_tokens_do_not_replace_existing_login() {
    let store = MemoryStore::default();
    store.set(&github(), &token("previous-token")).unwrap();
    let mut api = fake_api(vec![]);
    api.verified = false;
    assert!(matches!(
        verify_and_save(&api, &store, &github(), &token("rejected-token")),
        Err(AuthError::Rejected)
    ));
    assert_eq!(store.writes.get(), 1);
    assert_eq!(
        store.get(&github()).unwrap().unwrap().expose(),
        "previous-token"
    );
    api.verified = true;
    assert_eq!(
        verify_and_save(&api, &store, &github(), &token("verified-token")).unwrap(),
        "octocat"
    );
    assert_eq!(
        store.get(&github()).unwrap().unwrap().expose(),
        "verified-token"
    );
}

#[test]
fn unavailable_store_does_not_silently_fall_back_to_plaintext() {
    let store = MemoryStore {
        unavailable: true,
        ..Default::default()
    };
    assert!(matches!(
        verify_and_save(&fake_api(vec![]), &store, &github(), &token("valid-token")),
        Err(AuthError::SecureStore)
    ));
    assert_eq!(store.writes.get(), 0);
    assert!(store.values.borrow().is_empty());
}

#[test]
fn stdin_token_input_refuses_visible_terminal_entry_and_rejects_truncation() {
    assert!(matches!(
        read_stdin_token(true, "secret".as_bytes()),
        Err(AuthError::TokenInputTerminal)
    ));
    assert_eq!(
        read_stdin_token(false, "secret\n".as_bytes())
            .unwrap()
            .expose(),
        "secret"
    );
    assert!(matches!(
        read_stdin_token(false, "secret\nsecond-token".as_bytes()),
        Err(AuthError::InvalidToken)
    ));
    assert!(read_stdin_token(false, vec![b'x'; 20_000].as_slice()).is_err());
}

#[test]
fn auth_status_json_exposes_account_and_source_only() {
    let status = AuthStatus {
        schema_version: "grit.auth/v1",
        hostname: "github.com",
        account: "octocat",
        source: CredentialSource::Grit,
        authenticated: true,
        expires_at: None,
    };
    assert_eq!(
        serde_json::to_value(status).unwrap(),
        json!({"schema_version":"grit.auth/v1", "hostname":"github.com", "account":"octocat", "source":"grit", "authenticated":true})
    );
}

#[test]
fn native_http_flow_sends_expected_form_and_verifies_account_before_saving() {
    let mut server = Server::new();
    let begin = server.mock("POST", "/login/device/code").match_header("accept", "application/json")
        .match_body(Matcher::AllOf(vec![Matcher::UrlEncoded("client_id".to_owned(), "our-client-id".to_owned()), Matcher::UrlEncoded("scope".to_owned(), "repo".to_owned())]))
        .with_status(200).with_body(json!({"device_code":"secret-device-code", "user_code":"CODE-ABCD", "verification_uri":"https://github.com/login/device", "expires_in":900, "interval":5}).to_string()).create();
    let poll = server
        .mock("POST", "/login/oauth/access_token")
        .match_body(Matcher::AllOf(vec![
            Matcher::UrlEncoded("client_id".to_owned(), "our-client-id".to_owned()),
            Matcher::UrlEncoded("device_code".to_owned(), "secret-device-code".to_owned()),
            Matcher::UrlEncoded(
                "grant_type".to_owned(),
                "urn:ietf:params:oauth:grant-type:device_code".to_owned(),
            ),
        ]))
        .with_status(200)
        .with_body(r#"{"access_token":"fresh-token","token_type":"bearer","scope":"repo"}"#)
        .create();
    let user = server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer fresh-token")
        .with_status(200)
        .with_body(r#"{"login":"octocat"}"#)
        .create();
    let api = GitHubAuth::for_test(&server.url());
    let token = device::authorize(
        &api,
        &mut FakeClock::default(),
        &github(),
        "our-client-id",
        |_| {},
    )
    .unwrap();
    let store = MemoryStore::default();
    assert_eq!(
        verify_and_save(&api, &store, &github(), &token).unwrap(),
        "octocat"
    );
    assert_eq!(
        store.get(&github()).unwrap().unwrap().expose(),
        "fresh-token"
    );
    begin.assert();
    poll.assert();
    user.assert();
}

#[test]
fn authentication_http_redirects_do_not_forward_bearer_tokens() {
    let mut destination = Server::new();
    let forbidden = destination
        .mock("GET", "/stolen")
        .expect(0)
        .with_status(200)
        .with_body(r#"{"login":"attacker"}"#)
        .create();
    let mut server = Server::new();
    let redirect = server
        .mock("GET", "/user")
        .with_status(302)
        .with_header("location", &format!("{}/stolen", destination.url()))
        .create();
    let api = GitHubAuth::for_test(&server.url());
    assert!(matches!(
        api.account(&token("secret-token")),
        Err(AuthError::Http(302))
    ));
    redirect.assert();
    forbidden.assert();
}

#[test]
fn untrusted_verification_links_and_terminal_control_codes_are_rejected() {
    for (uri, user_code) in [
        ("https://attacker.example/login/device", "CODE-ABCD"),
        ("http://github.com/login/device", "CODE-ABCD"),
        ("https://github.com/login/device?token=secret", "CODE-ABCD"),
        ("https://github.com/login/device", "CODE-\u{1b}[31m"),
    ] {
        let mut server = Server::new();
        let begin = server.mock("POST", "/login/device/code").with_status(200).with_body(json!({"device_code":"secret-device-code", "user_code":user_code, "verification_uri":uri, "expires_in":900, "interval":5}).to_string()).create();
        let api = GitHubAuth::for_test(&server.url());
        assert!(matches!(
            device::authorize(
                &api,
                &mut FakeClock::default(),
                &github(),
                "our-client-id",
                |_| panic!("must not display untrusted response")
            ),
            Err(AuthError::Response)
        ));
        begin.assert();
    }
}

#[test]
fn oauth_denial_expiry_and_configuration_errors_are_actionable_and_redacted() {
    for (error, expected) in [
        ("access_denied", "denied"),
        ("expired_token", "expired"),
        ("device_flow_disabled", "disabled"),
        ("incorrect_client_credentials", "client ID"),
        ("server-secret-unknown", "could not authorize"),
    ] {
        let mut server = Server::new();
        let response = server
            .mock("POST", "/login/oauth/access_token")
            .with_status(200)
            .with_body(
                json!({"error":error, "error_description":"DO-NOT-ECHO-secret-token"}).to_string(),
            )
            .create();
        let api = GitHubAuth::for_test(&server.url());
        let error = match api.poll(
            "our-client-id",
            "secret-device-code",
            Duration::from_secs(1),
        ) {
            Err(error) => error,
            Ok(_) => panic!("accepted an error"),
        };
        assert!(error.to_string().contains(expected));
        assert!(!format!("{error:?} {error}").contains("DO-NOT-ECHO"));
        response.assert();
    }
}

#[test]
fn server_failures_and_malformed_responses_never_echo_response_bodies() {
    for (status, body) in [
        (401, "secret-token"),
        (503, "secret-token"),
        (200, "{\"login\": \"secret-token\""),
    ] {
        let mut server = Server::new();
        let response = server
            .mock("GET", "/user")
            .with_status(status)
            .with_body(body)
            .create();
        let error = GitHubAuth::for_test(&server.url())
            .account(&token("secret-token"))
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret-token"));
        response.assert();
    }
}

#[test]
fn secure_envelope_preserves_expiry_and_expired_login_requires_reauthentication() {
    let current = token("expiring-token").with_lifetime(Some(3600)).unwrap();
    let encoded = store::encode_credential(&current).unwrap();
    let decoded = store::decode_credential(encoded).unwrap();
    assert_eq!(decoded.expires_at, current.expires_at);
    assert_eq!(decoded.expose(), "expiring-token");
    assert!(decoded.ensure_current().is_ok());
    let mut expired = token("expired-token");
    expired.expires_at = Some(1);
    assert!(matches!(
        discover_with(None, || Ok(Some(expired))),
        Err(AuthError::CredentialExpired)
    ));
    assert!(matches!(
        token("expired-token").with_lifetime(Some(0)),
        Err(AuthError::CredentialExpired)
    ));
    assert!(matches!(
        token("overflow").with_lifetime(Some(u64::MAX)),
        Err(AuthError::Response)
    ));
}

#[test]
fn device_http_expiry_is_recorded_without_persisting_refresh_tokens() {
    let mut server = Server::new();
    let response = server.mock("POST", "/login/oauth/access_token").with_status(200)
        .with_body(r#"{"access_token":"short-lived-token","token_type":"bearer","expires_in":28800,"refresh_token":"unused-secret-refresh-token","refresh_token_expires_in":15897600}"#).create();
    let before = now_seconds();
    let Poll::Authorized(token) = GitHubAuth::for_test(&server.url())
        .poll("client", "device", Duration::from_secs(1))
        .unwrap()
    else {
        panic!("not authorized")
    };
    let expiry = token.expires_at.unwrap();
    assert!(expiry >= before + 28800 && expiry <= now_seconds() + 28800);
    let saved = store::encode_credential(&token).unwrap();
    assert!(!saved.contains("refresh"));
    response.assert();
}
