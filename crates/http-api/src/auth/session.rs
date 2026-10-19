//! Session resolution for every request, and the principal it yields.
//!
//! With authentication enabled the middleware is default-deny: a path is open
//! only when [`access_for`] says so. A session counts only while its sealed
//! cookie verifies, its provider session is live (or refreshes), and the user
//! row it names is `active` in PostgreSQL right now. That last read has no
//! cache in front of it, so deactivation takes effect on the next request.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts, Request, State},
    http::{header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use deepref_app::identity::{
    SessionUpdate, StoredBrowserSession, UserRecord, find_active_session, update_active_session,
};
use deepref_application::identity::{IdentityError, OpaqueSecret, RefreshOutcome, SessionStatus};
use deepref_domain::Principal;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::{AuthRuntime, read_cookie};
use crate::{
    error::{ApiError, AuthFailure},
    state::AppState,
};

/// How long a refresh result answers other requests that carry the same,
/// now-rotated, cookie. A page load fires many requests at once; the provider
/// accepts each refresh token once, so only the first may call it.
const REFRESH_REUSE_WINDOW: Duration = Duration::from_secs(30);

/// How many times one request re-reads a session another request rotated
/// under it before giving up with a retryable error.
const MAX_PERSIST_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    /// No session is read: health probes, the OpenAPI document, the sign-in
    /// redirects that run before a session exists, sign-out (which revokes by
    /// cookie digest and so works while the provider is down), and the
    /// machine-to-machine webhook deliveries that authenticate by their own
    /// token and signature.
    Open,
    Required,
}

fn access_for(path: &str) -> Access {
    match path {
        "/health" | "/openapi.json" | "/auth/login" | "/auth/callback" | "/auth/logout" => {
            Access::Open
        }
        _ if path.starts_with("/health/") || is_machine_hook(path) => Access::Open,
        _ => Access::Required,
    }
}

/// Machine-authenticated deliveries: a workflow's webhook and its inbound-email
/// address. Callers are external services with no browser session; each route
/// checks its private address token (and, for webhooks, the HMAC signature)
/// itself. This list is an explicit allowlist: exactly `<prefix><one segment>`,
/// nothing nested and nothing else.
const MACHINE_HOOK_PREFIXES: [&str; 2] = ["/hooks/workflows/", "/hooks/email/"];

fn is_machine_hook(path: &str) -> bool {
    MACHINE_HOOK_PREFIXES.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|token| !token.is_empty() && !token.contains('/'))
    })
}

/// The authenticated request: who, and the session they used.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub principal: Principal,
    pub user: UserRecord,
}

/// Extractor for the request's principal. Rejects with 401 when the request
/// carries no valid session, and when authentication is not enabled.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedPrincipal(pub Principal);

/// Extractor for the full session context.
#[derive(Debug, Clone)]
pub struct CurrentSession(pub SessionContext);

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<SessionContext>()
            .map(|session| Self(session.principal))
            .ok_or(ApiError::Auth(AuthFailure::Unauthenticated))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for CurrentSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<SessionContext>()
            .cloned()
            .map(Self)
            .ok_or(ApiError::Auth(AuthFailure::Unauthenticated))
    }
}

impl<S: Send + Sync> OptionalFromRequestParts<S> for CurrentSession {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<SessionContext>().cloned().map(Self))
    }
}

#[derive(Clone)]
pub(crate) struct SessionGate {
    pub(crate) enabled: bool,
    pub(crate) state: AppState,
}

enum Resolution {
    Anonymous,
    Rejected(&'static str),
    Authenticated(Box<SessionContext>),
}

pub(crate) async fn authenticate_requests(
    State(gate): State<SessionGate>,
    mut request: Request,
    next: Next,
) -> Response {
    if !gate.enabled {
        return next.run(request).await;
    }
    // `x-actor-*` headers are legacy scientific attribution only (read by
    // `extract_actor` until 03c). Nothing here reads them, and they are left
    // untouched: identity is the session's `UserId`, carried as an extension.
    let access = access_for(request.uri().path());
    if access == Access::Open {
        return next.run(request).await;
    }
    let Some(auth) = gate.state.auth.clone() else {
        return ApiError::Configuration("authentication is enabled but not configured".to_owned())
            .into_response();
    };
    let cookie_token = read_cookie(request.headers(), auth.cookies().session_name())
        .filter(|value| !value.is_empty());
    match resolve(&auth, &gate.state.pool, cookie_token).await {
        Ok(Resolution::Authenticated(session)) => {
            request.extensions_mut().insert(*session);
            next.run(request).await
        }
        Ok(Resolution::Anonymous) => ApiError::Auth(AuthFailure::Unauthenticated).into_response(),
        Ok(Resolution::Rejected(reason)) => {
            tracing::warn!(reason, "rejected a request with an unusable session");
            let mut response = ApiError::Auth(AuthFailure::SessionExpired).into_response();
            clear_session_cookie(&auth, &mut response);
            response
        }
        Err(error) => error.into_response(),
    }
}

fn clear_session_cookie(auth: &AuthRuntime, response: &mut Response) {
    if let Some(cookie) = auth.cookies().clear(auth.cookies().session_name()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
}

async fn resolve(
    auth: &AuthRuntime,
    pool: &PgPool,
    cookie_token: Option<&str>,
) -> Result<Resolution, ApiError> {
    let Some(cookie_token) = cookie_token else {
        return Ok(Resolution::Anonymous);
    };
    if Uuid::parse_str(cookie_token).is_err() {
        return Ok(Resolution::Rejected("malformed_session_cookie"));
    }
    let token_hash = auth.session_tokens().token_hash(cookie_token);
    for _ in 0..MAX_PERSIST_ATTEMPTS {
        let Some(stored) = find_active_session(pool, &token_hash).await? else {
            return Ok(Resolution::Rejected("unknown_expired_or_revoked_session"));
        };
        let (session, refreshed) = match auth
            .provider()
            .inspect_session(&stored.sealed_provider_session)
            .await
        {
            Ok(SessionStatus::Active(session)) => (session, None),
            Ok(SessionStatus::Expired(_)) => {
                match auth
                    .refresh()
                    .refresh(auth.provider(), &stored.sealed_provider_session)
                    .await
                    .map_err(provider_error)?
                {
                    RefreshOutcome::Refreshed {
                        sealed_session,
                        session,
                    } => (session, Some(sealed_session)),
                    RefreshOutcome::Rejected => {
                        // The provider spends a refresh token once. If another
                        // request or replica already rotated this session, its
                        // row moved on and that is the session to use.
                        if find_active_session(pool, &token_hash)
                            .await?
                            .is_some_and(|current| current.version != stored.version)
                        {
                            continue;
                        }
                        return Ok(Resolution::Rejected("refresh_rejected"));
                    }
                }
            }
            Ok(SessionStatus::Invalid) => return Ok(Resolution::Rejected("invalid_session")),
            Err(error) => return Err(provider_error(error)),
        };
        if session.external_user_id != stored.user.external_user_id {
            return Ok(Resolution::Rejected("session_user_mismatch"));
        }
        let session_ref = auth.session_refs().session_ref(&session.session_id);
        if !super::constant_time_eq(&session_ref, &stored.session_ref) {
            return Ok(Resolution::Rejected("session_reference_mismatch"));
        }
        if let Some(refreshed) = &refreshed {
            match update_active_session(pool, stored.id, stored.version, refreshed, &session_ref)
                .await?
            {
                SessionUpdate::Updated => {}
                SessionUpdate::Gone => {
                    return Ok(Resolution::Rejected("session_revoked_during_refresh"));
                }
                // Lost the race: this refresh is superseded. Reload and use
                // the winner's session instead of overwriting it.
                SessionUpdate::Conflict => continue,
            }
        }
        return Ok(Resolution::Authenticated(Box::new(context(stored))));
    }
    Err(ApiError::Auth(AuthFailure::ProviderUnavailable))
}

fn context(stored: StoredBrowserSession) -> SessionContext {
    SessionContext {
        principal: Principal::user(stored.user.id),
        user: stored.user,
    }
}

fn provider_error(error: IdentityError) -> ApiError {
    match error {
        IdentityError::Unavailable => ApiError::Auth(AuthFailure::ProviderUnavailable),
        IdentityError::Rejected | IdentityError::Internal(_) => {
            ApiError::Auth(AuthFailure::SessionExpired)
        }
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<(Instant, RefreshOutcome)>>>;

/// Single-flight refresh keyed by the cookie being refreshed.
#[derive(Default)]
pub(crate) struct RefreshCoalescer {
    slots: Mutex<HashMap<[u8; 32], Slot>>,
}

impl RefreshCoalescer {
    pub(crate) async fn refresh(
        &self,
        provider: &dyn deepref_application::identity::IdentityProvider,
        sealed: &OpaqueSecret,
    ) -> Result<RefreshOutcome, IdentityError> {
        let key: [u8; 32] = Sha256::digest(sealed.expose().as_bytes()).into();
        let slot = {
            let mut slots = self
                .slots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Drop settled entries nobody is waiting on.
            slots.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || slot
                        .try_lock()
                        .map(|cached| {
                            cached
                                .as_ref()
                                .is_some_and(|(at, _)| at.elapsed() < REFRESH_REUSE_WINDOW)
                        })
                        .unwrap_or(true)
            });
            Arc::clone(slots.entry(key).or_default())
        };
        let mut cached = slot.lock().await;
        if let Some((at, outcome)) = cached.as_ref()
            && at.elapsed() < REFRESH_REUSE_WINDOW
        {
            return Ok(outcome.clone());
        }
        let outcome = provider.refresh_session(sealed).await?;
        *cached = Some((Instant::now(), outcome.clone()));
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_paths_are_open() {
        for open in [
            "/health",
            "/health/live",
            "/health/ready",
            "/openapi.json",
            "/auth/login",
            "/auth/callback",
            "/auth/logout",
            "/hooks/workflows/0123abcd",
            "/hooks/email/0123abcd",
        ] {
            assert_eq!(access_for(open), Access::Open, "{open}");
        }
        for protected in [
            "/me",
            "/projects",
            "/settings",
            "/healthz",
            "/health-x",
            "/hooks",
            "/hooks/",
            "/hooks/workflows/",
            "/hooks/workflows",
            "/hooks/workflows/token/extra",
            "/hooks/other/token",
            "/hooks/emailx/token",
            "/",
        ] {
            assert_eq!(access_for(protected), Access::Required, "{protected}");
        }
    }
}
