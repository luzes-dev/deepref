//! Bounded audit of authentication rejections (handoff 11 Part B).
//!
//! Authentication rejections are attacker-controlled: a flood of bad cookies,
//! replayed callbacks or cross-origin posts must not saturate audit storage.
//! The bound is the one `authorization.denied` uses (03b): sample one row per
//! (stage, reason, target user, one-minute window), claimed process-locally
//! and enforced across processes by `admin_audit_auth_rejection_window_idx`
//! (migration 0113) through `INSERT ... ON CONFLICT DO NOTHING`.
//!
//! Nothing a caller supplied is stored. `stage` and `reason` come from the
//! closed vocabularies below and `window` is a minute bucket; the target is a
//! user id read from a stored session row, never anything read from the
//! request, so an attacker cannot create new keys.

use std::{
    collections::HashSet,
    sync::{Mutex, MutexGuard},
};

use deepref_domain::UserId;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// Where authentication was refused. Each rejection reason belongs to exactly
/// one stage, so a wrong (stage, reason) pair cannot be expressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectionStage {
    ApiKey,
    /// The provider callback was refused.
    Callback,
    /// A request presented a session the server would not accept.
    Session,
    /// A session refresh ended the session.
    Refresh,
    /// The CSRF origin guard refused a cookie-authenticated unsafe request.
    Origin,
}

impl RejectionStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Callback => "callback",
            Self::Session => "session",
            Self::Refresh => "refresh",
            Self::Origin => "origin",
        }
    }
}

/// Why authentication was refused: a closed vocabulary, and nothing a caller
/// typed. The database enforces the same closed lists per stage (migration
/// 0141).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectionReason {
    ApiKeyInvalidCredential,
    ApiKeyRouteDenied,
    // Callback.
    CallbackInvalidState,
    CallbackAccessDenied,
    CallbackInvalidCode,
    CallbackProviderUnavailable,
    CallbackProvisioningFailed,
    CallbackAccountDeactivated,
    // Session.
    SessionMalformedCookie,
    SessionUnknownExpiredOrRevoked,
    SessionUserMismatch,
    SessionReferenceMismatch,
    SessionInvalid,
    // Refresh.
    RefreshRejected,
    RefreshRevokedDuringRefresh,
    RefreshUserMismatch,
    RefreshReferenceMismatch,
    // Origin guard.
    OriginMultipleHeaders,
    OriginMismatch,
    OriginSecFetchSiteMismatch,
    OriginMissingEvidence,
}

impl RejectionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKeyInvalidCredential => "invalid_credential",
            Self::ApiKeyRouteDenied => "service_route_denied",
            Self::CallbackInvalidState => "invalid_state",
            Self::CallbackAccessDenied => "access_denied",
            Self::CallbackInvalidCode => "invalid_code",
            Self::CallbackProviderUnavailable => "provider_unavailable",
            Self::CallbackProvisioningFailed => "provisioning_failed",
            Self::CallbackAccountDeactivated => "account_deactivated",
            Self::SessionMalformedCookie => "malformed_session_cookie",
            Self::SessionUnknownExpiredOrRevoked => "unknown_expired_or_revoked_session",
            Self::SessionUserMismatch => "session_user_mismatch",
            Self::SessionReferenceMismatch => "session_reference_mismatch",
            Self::SessionInvalid => "invalid_session",
            Self::RefreshRejected => "refresh_rejected",
            Self::RefreshRevokedDuringRefresh => "session_revoked_during_refresh",
            Self::RefreshUserMismatch => "session_user_mismatch",
            Self::RefreshReferenceMismatch => "session_reference_mismatch",
            Self::OriginMultipleHeaders => "multiple_origin_headers",
            Self::OriginMismatch => "origin_mismatch",
            Self::OriginSecFetchSiteMismatch => "sec_fetch_site_mismatch",
            Self::OriginMissingEvidence => "missing_origin_evidence",
        }
    }

    pub const fn stage(self) -> RejectionStage {
        match self {
            Self::ApiKeyInvalidCredential | Self::ApiKeyRouteDenied => RejectionStage::ApiKey,
            Self::CallbackInvalidState
            | Self::CallbackAccessDenied
            | Self::CallbackInvalidCode
            | Self::CallbackProviderUnavailable
            | Self::CallbackProvisioningFailed
            | Self::CallbackAccountDeactivated => RejectionStage::Callback,
            Self::SessionMalformedCookie
            | Self::SessionUnknownExpiredOrRevoked
            | Self::SessionUserMismatch
            | Self::SessionReferenceMismatch
            | Self::SessionInvalid => RejectionStage::Session,
            Self::RefreshRejected
            | Self::RefreshRevokedDuringRefresh
            | Self::RefreshUserMismatch
            | Self::RefreshReferenceMismatch => RejectionStage::Refresh,
            Self::OriginMultipleHeaders
            | Self::OriginMismatch
            | Self::OriginSecFetchSiteMismatch
            | Self::OriginMissingEvidence => RejectionStage::Origin,
        }
    }
}

/// One refused authentication, reduced to what may be stored.
#[derive(Debug, Clone, Copy)]
pub struct AuthRejection<'a> {
    pub reason: RejectionReason,
    /// The user whose stored session was refused, when a session row already
    /// named one. Never read from the request.
    pub target: Option<UserId>,
    /// The correlation ID, when the rejection happened inside a request.
    pub request_id: Option<&'a str>,
}

/// Records `auth.rejected` into `admin_audit_events`, bounded two ways.
///
/// The audit table is append-only, so a count cannot be bumped in place. The
/// writer samples: one row per (stage, reason, target user, one-minute
/// window). A process-local set skips the round trip for keys it already
/// wrote this window, and `admin_audit_auth_rejection_window_idx` (0113)
/// makes the bound hold across processes. A flood therefore stores at most
/// (stages x reasons) rows per identified user per minute, plus one row per
/// (stage, reason) per minute for the anonymous flood case.
pub struct RejectionAuditor {
    pool: PgPool,
    seen: Mutex<SeenWindow>,
}

#[derive(Default)]
struct SeenWindow {
    window: i64,
    keys: HashSet<(&'static str, &'static str, Uuid)>,
}

/// Process-local cap. The key space is tiny (closed vocabularies x users);
/// past this many entries the set is simply cleared, which only costs extra
/// no-op inserts.
const MAX_SEEN_KEYS: usize = 8192;

/// The key an anonymous rejection shares: no stored session named a user.
const ANONYMOUS_TARGET: Uuid = Uuid::nil();

impl RejectionAuditor {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            seen: Mutex::new(SeenWindow::default()),
        }
    }

    fn window_now() -> i64 {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        i64::try_from(seconds / 60).unwrap_or(0)
    }

    fn key(rejection: &AuthRejection<'_>) -> (&'static str, &'static str, Uuid) {
        (
            rejection.reason.stage().as_str(),
            rejection.reason.as_str(),
            rejection
                .target
                .map_or(ANONYMOUS_TARGET, |user| user.as_uuid()),
        )
    }

    /// Forgets a claim whose insert failed, so the next rejection retries it.
    fn release(&self, rejection: &AuthRejection<'_>, window: i64) {
        let mut seen = self.lock_seen();
        if seen.window == window {
            seen.keys.remove(&Self::key(rejection));
        }
    }

    /// Whether this process has not yet written the key in `window`; if so,
    /// claims it. The claim stands only once the insert succeeds (a unique
    /// conflict counts: the row exists).
    fn claim(&self, rejection: &AuthRejection<'_>, window: i64) -> bool {
        let mut seen = self.lock_seen();
        if seen.window != window || seen.keys.len() >= MAX_SEEN_KEYS {
            seen.window = window;
            seen.keys.clear();
        }
        seen.keys.insert(Self::key(rejection))
    }

    fn lock_seen(&self) -> MutexGuard<'_, SeenWindow> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns whether a row was written. A failed write is the caller's to
    /// log; a rejection is never turned into an error by its own audit.
    pub async fn record(&self, rejection: AuthRejection<'_>) -> Result<bool, sqlx::Error> {
        let window = Self::window_now();
        if !self.claim(&rejection, window) {
            return Ok(false);
        }
        let metadata = json!({
            "stage": rejection.reason.stage().as_str(),
            "reason": rejection.reason.as_str(),
            "window": window.to_string(),
        });
        let written = match sqlx::query(
            "INSERT INTO admin_audit_events \
               (id, event_type, principal_kind, target_user_id, request_id, metadata) \
             VALUES ($1, 'auth.rejected', 'system', $2, $3, $4) \
             ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(rejection.target.map(UserId::as_uuid))
        .bind(rejection.request_id.filter(|id| id.len() <= 200))
        .bind(metadata)
        .execute(&self.pool)
        .await
        {
            Ok(result) => result.rows_affected(),
            Err(error) => {
                self.release(&rejection, window);
                return Err(error);
            }
        };
        Ok(written == 1)
    }
}
