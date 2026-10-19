//! Browser authentication: the identity runtime, the session middleware and the
//! principal extractor.
//!
//! The server is the authority. Nothing here reads `x-actor-kind`,
//! `x-actor-id` or any browser storage value.

mod cookies;
mod session;

use std::sync::Arc;

use deepref_application::identity::{IdentityProvider, SessionRefKey, SessionTokenKey};
use deepref_config::PublicOrigin;

pub(crate) use cookies::{
    CookiePolicy, LOGIN_COOKIE_MAX_AGE_SECS, SESSION_COOKIE_MAX_AGE_SECS, read_cookie,
};
pub use session::{AuthenticatedPrincipal, CurrentSession, SessionContext};
pub(crate) use session::{SessionGate, authenticate_requests};

/// The browser reaches the API under this prefix (the Vite dev server and the
/// production proxy both strip it), so provider redirect URIs include it.
pub const API_PUBLIC_PREFIX: &str = "/api";

#[derive(Clone)]
pub struct AuthRuntime {
    inner: Arc<Inner>,
}

struct Inner {
    provider: Arc<dyn IdentityProvider>,
    origin: String,
    cookies: CookiePolicy,
    session_refs: SessionRefKey,
    session_tokens: SessionTokenKey,
    refresh: session::RefreshCoalescer,
}

impl AuthRuntime {
    /// `session_ref_secret` keys the audit hash of provider session IDs.
    pub fn new(
        provider: Arc<dyn IdentityProvider>,
        origin: &PublicOrigin,
        session_ref_secret: &str,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                provider,
                origin: origin.as_str().to_owned(),
                cookies: CookiePolicy::for_origin(origin),
                session_refs: SessionRefKey::from_secret(session_ref_secret),
                session_tokens: SessionTokenKey::from_secret(session_ref_secret),
                refresh: session::RefreshCoalescer::default(),
            }),
        }
    }

    pub(crate) fn provider(&self) -> &dyn IdentityProvider {
        self.inner.provider.as_ref()
    }

    pub(crate) fn origin(&self) -> &str {
        &self.inner.origin
    }

    pub(crate) fn cookies(&self) -> CookiePolicy {
        self.inner.cookies
    }

    pub(crate) fn session_refs(&self) -> &SessionRefKey {
        &self.inner.session_refs
    }

    pub(crate) fn session_tokens(&self) -> &SessionTokenKey {
        &self.inner.session_tokens
    }

    pub(crate) fn callback_url(&self) -> String {
        format!("{}{API_PUBLIC_PREFIX}/auth/callback", self.inner.origin)
    }

    pub(crate) fn refresh(&self) -> &session::RefreshCoalescer {
        &self.inner.refresh
    }
}

/// Compares two byte strings without stopping at the first difference.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
