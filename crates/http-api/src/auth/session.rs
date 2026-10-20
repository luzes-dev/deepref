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
    RefreshLease, SessionUpdate, StoredBrowserSession, UserRecord, claim_refresh_lease,
    development_principal_id, end_session_if_leased, find_active_session, release_refresh_lease,
    update_active_session,
};
use deepref_application::identity::{
    IdentityError, OpaqueSecret, ProviderSession, RefreshOutcome, SessionStatus,
};
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

/// How long a refresh lease protects a refresh from other requests. It covers
/// the provider call and the WorkOS replay grace, and frees a session whose
/// refresher died.
pub(super) const REFRESH_LEASE: Duration = Duration::from_secs(30);

/// How often a request that lost the lease checks for the winner's commit:
/// starts here and doubles up to [`REFRESH_POLL_MAX`].
const REFRESH_POLL: Duration = Duration::from_millis(100);
const REFRESH_POLL_MAX: Duration = Duration::from_secs(1);

/// Longest a lease release may take; the lease expires on its own anyway.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Whether the session layer reads no session for this concrete path. The
/// policy table's `Open` rows must match it exactly.
#[cfg(test)]
pub(crate) fn is_session_exempt(path: &str) -> bool {
    access_for(path) == Access::Open
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
            .get::<Principal>()
            .copied()
            .map(Self)
            .or_else(|| {
                parts
                    .extensions
                    .get::<SessionContext>()
                    .map(|session| Self(session.principal))
            })
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
    pub(crate) development: bool,
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
        if gate.development {
            request
                .extensions_mut()
                .insert(Principal::user(development_principal_id()));
        }
        return next.run(request).await;
    }
    // N-1 browsers may still send `x-actor-*`; attribution comes only from the
    // active session principal carried as a request extension.
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
            request.extensions_mut().insert(session.principal);
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
        match auth
            .provider()
            .inspect_session(&stored.sealed_provider_session)
            .await
        {
            Ok(SessionStatus::Active(session)) => {
                if let Some(reason) = identity_mismatch(auth, &stored, &session) {
                    return Ok(Resolution::Rejected(reason));
                }
            }
            Ok(SessionStatus::Expired(_)) => match refresh_expired(auth, pool, &stored).await? {
                Refresh::Persisted => {}
                Refresh::Superseded => continue,
                Refresh::Terminal(reason) => return Ok(Resolution::Rejected(reason)),
            },
            Ok(SessionStatus::Invalid) => return Ok(Resolution::Rejected("invalid_session")),
            Err(error) => return Err(provider_error(error)),
        }
        return Ok(Resolution::Authenticated(Box::new(context(stored))));
    }
    Err(ApiError::Auth(AuthFailure::ProviderUnavailable))
}

/// Why a provider session does not belong to the stored row, if it does not.
fn identity_mismatch(
    auth: &AuthRuntime,
    stored: &StoredBrowserSession,
    session: &ProviderSession,
) -> Option<&'static str> {
    if session.external_user_id != stored.user.external_user_id {
        return Some("session_user_mismatch");
    }
    let session_ref = auth.session_refs().session_ref(&session.session_id);
    (!super::constant_time_eq(&session_ref, &stored.session_ref))
        .then_some("session_reference_mismatch")
}

enum Refresh {
    /// The renewed session is stored.
    Persisted,
    /// Another request refreshed first: reload and use its session.
    Superseded,
    /// The session is definitively over. Only this clears the cookie.
    Terminal(&'static str),
}

/// Refreshes an expired session as the only request doing so, across replicas.
/// Whoever takes the row's refresh lease spends the token; everyone else waits
/// for that commit. Contention that does not resolve in time is a retryable
/// provider error, never a verdict on the session.
///
/// Within this process, requests for one session queue on a gate and only the
/// gate's holder queries the lease, so N waiters cost one poller, not N. The
/// refresh itself is bounded by a budget well inside the lease, so ownership
/// holds from the provider call through the commit.
async fn refresh_expired(
    auth: &AuthRuntime,
    pool: &PgPool,
    stored: &StoredBrowserSession,
) -> Result<Refresh, ApiError> {
    let unavailable = || ApiError::Auth(AuthFailure::ProviderUnavailable);
    let owner = Uuid::new_v4();
    let started = tokio::time::Instant::now();
    let deadline = started + auth.refresh_wait();
    let gate = auth.refresh_gate(stored.id);
    let Ok(mut state) = tokio::time::timeout_at(deadline, gate.lock_owned()).await else {
        tracing::warn!("a competing session refresh did not finish in time");
        return Err(unavailable());
    };
    // A request ahead of us in this process already stored a newer session.
    if state
        .settled
        .is_some_and(|version| version >= stored.version)
    {
        return Ok(Refresh::Superseded);
    }
    // The holder ahead of us waited out the same contention and gave up after
    // we began waiting: its answer is ours.
    if state.gave_up.is_some_and(|at| at >= started) {
        return Err(unavailable());
    }
    let mut backoff = REFRESH_POLL;
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("a competing session refresh did not finish in time");
            state.gave_up = Some(tokio::time::Instant::now());
            return Err(unavailable());
        }
        auth.count_lease_query();
        let claim = tokio::time::timeout_at(
            deadline,
            claim_refresh_lease(pool, stored.id, stored.version, owner, auth.refresh_lease()),
        )
        .await;
        match claim {
            Err(_) => {
                tracing::warn!("claiming a session refresh lease did not finish in time");
                state.gave_up = Some(tokio::time::Instant::now());
                return Err(unavailable());
            }
            Ok(claim) => match claim? {
                RefreshLease::Claimed => break,
                RefreshLease::Superseded => return Ok(Refresh::Superseded),
                RefreshLease::Gone => {
                    return Ok(Refresh::Terminal("session_revoked_during_refresh"));
                }
                RefreshLease::Held if tokio::time::Instant::now() >= deadline => {
                    tracing::warn!("a competing session refresh did not finish in time");
                    state.gave_up = Some(tokio::time::Instant::now());
                    return Err(unavailable());
                }
                RefreshLease::Held => {
                    tokio::time::sleep_until((tokio::time::Instant::now() + backoff).min(deadline))
                        .await;
                    backoff = (backoff * 2).min(REFRESH_POLL_MAX);
                }
            },
        }
    }
    let outcome = match tokio::time::timeout(
        auth.refresh_budget(),
        refresh_and_persist(auth, pool, stored, owner),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::warn!("a session refresh exceeded its budget and was abandoned");
            Err(unavailable())
        }
    };
    if matches!(outcome, Ok(Refresh::Persisted)) {
        state.settled = Some(stored.version);
    } else {
        match tokio::time::timeout(
            RELEASE_TIMEOUT,
            release_refresh_lease(pool, stored.id, owner),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "could not release a session refresh lease"),
            Err(_) => tracing::warn!("releasing a session refresh lease timed out"),
        }
    }
    outcome
}

async fn refresh_and_persist(
    auth: &AuthRuntime,
    pool: &PgPool,
    stored: &StoredBrowserSession,
    owner: Uuid,
) -> Result<Refresh, ApiError> {
    match auth
        .refresh()
        .refresh(auth.provider(), &stored.sealed_provider_session)
        .await
        .map_err(provider_error)?
    {
        RefreshOutcome::Refreshed {
            sealed_session,
            session,
        } => {
            if let Some(reason) = identity_mismatch(auth, stored, &session) {
                return Ok(Refresh::Terminal(reason));
            }
            let session_ref = auth.session_refs().session_ref(&session.session_id);
            Ok(
                match update_active_session(
                    pool,
                    stored.id,
                    stored.version,
                    owner,
                    &sealed_session,
                    &session_ref,
                )
                .await?
                {
                    SessionUpdate::Updated => Refresh::Persisted,
                    SessionUpdate::Gone => Refresh::Terminal("session_revoked_during_refresh"),
                    // Lost the lease or the version: reload the winner's row.
                    SessionUpdate::Conflict => Refresh::Superseded,
                },
            )
        }
        // A rejection ends the session only if this request still owns the
        // lease and the row is unchanged, decided in the same statement that
        // revokes. Otherwise the rejection may be a replay of a token another
        // refresh already rotated, and that refresh's result is the truth.
        RefreshOutcome::Rejected => Ok(
            if end_session_if_leased(pool, stored.id, stored.version, owner).await? {
                Refresh::Terminal("refresh_rejected")
            } else {
                Refresh::Superseded
            },
        ),
    }
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

/// Per-session gate within one process. Remembers how the last holder fared so
/// queued requests inherit it instead of querying again.
pub(crate) type Gate = Arc<tokio::sync::Mutex<GateState>>;

#[derive(Default)]
pub(crate) struct GateState {
    /// Row version of the last refresh this process persisted.
    settled: Option<i64>,
    /// When the last holder gave up waiting on another replica's refresh.
    gave_up: Option<tokio::time::Instant>,
}

type Slot = Arc<tokio::sync::Mutex<Option<(Instant, RefreshOutcome)>>>;

/// Single-flight refresh keyed by the cookie being refreshed.
#[derive(Default)]
pub(crate) struct RefreshCoalescer {
    slots: Mutex<HashMap<[u8; 32], Slot>>,
    gates: Mutex<HashMap<Uuid, Gate>>,
    lease_queries: std::sync::atomic::AtomicU64,
}

impl RefreshCoalescer {
    pub(crate) fn gate(&self, session_id: Uuid) -> Gate {
        let mut gates = self
            .gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Drop gates nobody holds or waits on.
        gates.retain(|_, gate| Arc::strong_count(gate) > 1);
        Arc::clone(gates.entry(session_id).or_default())
    }

    pub(crate) fn count_lease_query(&self) {
        self.lease_queries
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn lease_queries(&self) -> u64 {
        self.lease_queries
            .load(std::sync::atomic::Ordering::Relaxed)
    }

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
