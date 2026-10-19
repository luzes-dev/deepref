//! The identity-provider port and the pure rules around browser sign-in.
//!
//! Provider DTOs stop in the adapter that implements [`IdentityProvider`]. The
//! application sees only the types declared here.

use std::{fmt, future::Future, pin::Pin};

use deepref_domain::ExternalUserId;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

pub type IdentityFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, IdentityError>> + Send + 'a>>;

/// Failures an identity provider adapter reports. None carries provider
/// payloads, tokens or cookie contents.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityError {
    #[error("the identity provider is unavailable")]
    Unavailable,
    #[error("the identity provider rejected the request")]
    Rejected,
    #[error("identity operation failed: {0}")]
    Internal(String),
}

/// A credential-bearing string. `Debug` prints nothing of it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueSecret(String);

impl OpaqueSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueSecret(<redacted>)")
    }
}

/// What the browser keeps between `/auth/login` and `/auth/callback`, sealed
/// in a short-lived cookie so the callback can prove it continues a login this
/// browser started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginState {
    pub state: String,
    pub code_verifier: OpaqueSecret,
    pub return_to: ReturnPath,
    pub expires_at_unix: i64,
}

#[derive(Debug, Clone)]
pub struct LoginStart {
    pub authorization_url: String,
    pub state: String,
    pub code_verifier: OpaqueSecret,
}

/// A provider session as DeepRef reads it: the provider's session ID and the
/// person it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSession {
    pub session_id: String,
    pub external_user_id: ExternalUserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProfile {
    pub email: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderLogin {
    pub sealed_session: OpaqueSecret,
    pub session: ProviderSession,
    pub profile: ProviderProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Active(ProviderSession),
    /// Genuine but past its access-token lifetime: a refresh may revive it.
    Expired(ProviderSession),
    /// Missing, tampered with, or not signed by the provider.
    Invalid,
}

#[derive(Debug, Clone)]
pub enum RefreshOutcome {
    Refreshed {
        sealed_session: OpaqueSecret,
        session: ProviderSession,
    },
    Rejected,
}

pub trait IdentityProvider: Send + Sync {
    /// Builds the provider sign-in URL with fresh `state` and PKCE values.
    fn begin_login(&self, redirect_uri: &str) -> Result<LoginStart, IdentityError>;

    /// Exchanges the callback's authorization code for a sealed session.
    fn complete_login<'a>(
        &'a self,
        code: &'a str,
        code_verifier: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, ProviderLogin>;

    /// Verifies a sealed session offline. Never errors: every failure is
    /// [`SessionStatus::Invalid`].
    fn inspect_session<'a>(&'a self, sealed: &'a OpaqueSecret)
    -> IdentityFuture<'a, SessionStatus>;

    fn refresh_session<'a>(
        &'a self,
        sealed: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, RefreshOutcome>;

    /// Ends a provider session. Callers treat failure as best effort.
    fn revoke_session<'a>(&'a self, session_id: &'a str) -> IdentityFuture<'a, ()>;

    fn list_session_ids<'a>(&'a self, user: &'a ExternalUserId) -> IdentityFuture<'a, Vec<String>>;

    fn seal_login_state(&self, state: &LoginState) -> Result<OpaqueSecret, IdentityError>;

    /// `None` for anything that is not a login state this provider sealed.
    fn open_login_state(&self, sealed: &str) -> Option<LoginState>;
}

/// Derives a 32-byte key for one purpose from a configured secret, so the
/// session-sealing key and the audit-hash key are unrelated.
pub fn derive_key(secret: &str, purpose: &str) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC accepts keys of any length"));
    mac.update(purpose.as_bytes());
    mac.finalize().into_bytes().into()
}

/// Keyed hash of a provider session ID. Audit rows hold this, never the ID.
#[derive(Clone)]
pub struct SessionRefKey([u8; 32]);

impl SessionRefKey {
    pub fn from_secret(secret: &str) -> Self {
        Self(derive_key(secret, "deepref/session-ref/v1"))
    }

    pub fn session_ref(&self, session_id: &str) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(&self.0)
            .unwrap_or_else(|_| unreachable!("HMAC accepts keys of any length"));
        mac.update(session_id.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
}

impl fmt::Debug for SessionRefKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionRefKey(<redacted>)")
    }
}

/// Keyed digest for the opaque browser session cookie. The raw cookie is never
/// stored in PostgreSQL, and this key is distinct from the provider-session
/// audit key above.
#[derive(Clone)]
pub struct SessionTokenKey([u8; 32]);

impl SessionTokenKey {
    pub fn from_secret(secret: &str) -> Self {
        Self(derive_key(secret, "deepref/session-cookie-lookup/v1"))
    }

    pub fn token_hash(&self, token: &str) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(&self.0)
            .unwrap_or_else(|_| unreachable!("HMAC accepts keys of any length"));
        mac.update(token.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
}

impl fmt::Debug for SessionTokenKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionTokenKey(<redacted>)")
    }
}

const MAX_RETURN_PATH_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReturnPathError {
    #[error("return target must be a path on this application")]
    NotAPath,
    #[error("return target is too long")]
    TooLong,
    #[error("return target contains forbidden characters")]
    ForbiddenCharacter,
}

/// Where to send a person after sign-in: a path (with optional query) on this
/// application, never a URL. Parsing refuses anything a browser could read as
/// another origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReturnPath(String);

impl ReturnPath {
    pub fn root() -> Self {
        Self("/".to_owned())
    }

    pub fn parse(raw: &str) -> Result<Self, ReturnPathError> {
        if raw.len() > MAX_RETURN_PATH_BYTES {
            return Err(ReturnPathError::TooLong);
        }
        // URL parsers drop tabs and newlines and treat `\` as `/`, so each of
        // these can turn `/x` into `//evil.example` after the check passes.
        if raw
            .chars()
            .any(|c| c.is_control() || c == '\\' || c.is_whitespace())
        {
            return Err(ReturnPathError::ForbiddenCharacter);
        }
        if !raw.starts_with('/') || raw.starts_with("//") {
            return Err(ReturnPathError::NotAPath);
        }
        // The API is reverse-proxied under /api; a return target is a page.
        if raw == "/api" || raw.starts_with("/api/") || raw.starts_with("/api?") {
            return Err(ReturnPathError::NotAPath);
        }
        let base = Url::parse("https://return-path.invalid")
            .unwrap_or_else(|_| unreachable!("static URL is valid"));
        let resolved = base.join(raw).map_err(|_| ReturnPathError::NotAPath)?;
        if resolved.origin() != base.origin() || !resolved.path().starts_with('/') {
            return Err(ReturnPathError::NotAPath);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ReturnPath {
    type Error = ReturnPathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ReturnPath> for String {
    fn from(value: ReturnPath) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_path_accepts_application_paths() {
        for ok in ["/", "/projects", "/projects/1?tab=a#x", "/a/b%20c"] {
            assert!(ReturnPath::parse(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn return_path_rejects_anything_that_can_leave_the_origin() {
        for bad in [
            "",
            "projects",
            "https://evil.example/",
            "//evil.example",
            "/\\evil.example",
            "/\t/evil.example",
            "/\n/evil.example",
            "/ /evil.example",
            "javascript:alert(1)",
            "/api/auth/login",
            "/api",
            "///evil.example",
        ] {
            assert!(ReturnPath::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            ReturnPath::parse(&format!("/{}", "a".repeat(MAX_RETURN_PATH_BYTES))),
            Err(ReturnPathError::TooLong)
        );
    }

    #[test]
    fn return_path_is_validated_when_deserialised() {
        assert!(serde_json::from_str::<ReturnPath>("\"//evil.example\"").is_err());
        assert!(serde_json::from_str::<ReturnPath>("\"/ok\"").is_ok());
    }

    #[test]
    fn session_refs_are_keyed_and_stable() {
        let a = SessionRefKey::from_secret("a-secret-of-sufficient-length-xxxxxxxx");
        let b = SessionRefKey::from_secret("another-secret-of-sufficient-length-x");
        assert_eq!(a.session_ref("sess_1"), a.session_ref("sess_1"));
        assert_ne!(a.session_ref("sess_1"), a.session_ref("sess_2"));
        assert_ne!(a.session_ref("sess_1"), b.session_ref("sess_1"));
        assert_eq!(a.session_ref("sess_1").len(), 32);
    }

    #[test]
    fn derived_keys_differ_by_purpose() {
        assert_ne!(derive_key("secret", "seal"), derive_key("secret", "audit"));
    }

    #[test]
    fn secrets_never_print() {
        let secret = OpaqueSecret::new("tok_super_secret");
        assert!(!format!("{secret:?}").contains("tok_super"));
    }
}
