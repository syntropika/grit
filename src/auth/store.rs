use super::{AuthError, AuthToken, Host};

// A dedicated namespace prevents logout from modifying other applications.
const SERVICE: &str = "org.syntropika.grit.github-auth";

pub(super) trait CredentialStore {
    fn get(&self, host: &Host) -> Result<Option<AuthToken>, AuthError>;
    fn set(&self, host: &Host, token: &AuthToken) -> Result<(), AuthError>;
    fn delete(&self, host: &Host) -> Result<bool, AuthError>;
}

pub(super) struct OsCredentialStore;

impl OsCredentialStore {
    pub(super) fn check(&self, host: &Host) -> Result<(), AuthError> {
        self.entry(host).map(|_| ())
    }

    fn entry(&self, host: &Host) -> Result<keyring::Entry, AuthError> {
        // Useful for headless sessions and hermetic tests; never fall back to plaintext.
        if std::env::var("GRIT_NO_KEYRING").is_ok_and(|value| value == "1") {
            return Err(AuthError::SecureStore);
        }
        keyring::Entry::new(SERVICE, &host.0).map_err(|_| AuthError::SecureStore)
    }
}

impl CredentialStore for OsCredentialStore {
    fn get(&self, host: &Host) -> Result<Option<AuthToken>, AuthError> {
        if std::env::var("GRIT_NO_KEYRING").is_ok_and(|value| value == "1") {
            return Ok(None);
        }
        match self.entry(host)?.get_password() {
            Ok(value) => decode_credential(zeroize::Zeroizing::new(value)).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AuthError::SecureStore),
        }
    }

    fn set(&self, host: &Host, token: &AuthToken) -> Result<(), AuthError> {
        self.entry(host)?
            .set_password(&encode_credential(token)?)
            .map_err(|_| AuthError::SecureStore)
    }

    fn delete(&self, host: &Host) -> Result<bool, AuthError> {
        match self.entry(host)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(_) => Err(AuthError::SecureStore),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedCredential {
    version: u8,
    access_token: String,
    expires_at: Option<u64>,
}

impl Drop for SavedCredential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
    }
}

pub(super) fn encode_credential(
    token: &AuthToken,
) -> Result<zeroize::Zeroizing<String>, AuthError> {
    let saved = SavedCredential {
        version: 1,
        access_token: token.expose().to_owned(),
        expires_at: token.expires_at,
    };
    serde_json::to_string(&saved)
        .map(zeroize::Zeroizing::new)
        .map_err(|_| AuthError::SecureStore)
}

pub(super) fn decode_credential(value: zeroize::Zeroizing<String>) -> Result<AuthToken, AuthError> {
    let saved: SavedCredential =
        serde_json::from_str(&value).map_err(|_| AuthError::SecureStore)?;
    if saved.version != 1 {
        return Err(AuthError::SecureStore);
    }
    let mut token = AuthToken::parse(&saved.access_token)?;
    token.expires_at = saved.expires_at;
    Ok(token)
}
