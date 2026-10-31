//! Users, sign-in provisioning and the administrative audit trail.
//!
//! Every audit row is written in the same transaction as the change it
//! records. Nothing here stores a cookie, token or raw provider session ID:
//! sessions appear only as the keyed hash `SessionRefKey` produces.

use deepref_application::identity::{
    IdentityProvider, OpaqueSecret, ProviderProfile, ProviderSession,
};
use deepref_domain::{ExternalUserId, UserId, UserStatus};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum IdentityStoreError {
    #[error("identity storage failed")]
    Database(#[from] sqlx::Error),
    #[error("stored user row is invalid: {0}")]
    Corrupt(&'static str),
    #[error("onboarding the signed-in user failed")]
    Workspace(#[from] crate::workspaces::WorkspaceError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    pub id: UserId,
    pub external_user_id: ExternalUserId,
    pub email: String,
    pub display_name: Option<String>,
    pub status: UserStatus,
}

/// Values stored with a newly issued browser session. The cookie itself is
/// deliberately absent; PostgreSQL receives only its keyed digest.
pub struct NewBrowserSession<'a> {
    pub token_hash: &'a [u8],
    pub sealed_provider_session: &'a OpaqueSecret,
    pub session_ref: &'a [u8],
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// The server-side session loaded for one request. Its provider credential is
/// already sealed by the WorkOS adapter and the cookie digest is not returned.
#[derive(Debug, Clone)]
pub struct StoredBrowserSession {
    pub id: Uuid,
    pub user: UserRecord,
    pub sealed_provider_session: OpaqueSecret,
    pub session_ref: Vec<u8>,
    /// Compare-and-set token for [`update_active_session`].
    pub version: i64,
}

/// What [`update_active_session`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionUpdate {
    Updated,
    /// Another request persisted a newer session first. Reload and use that.
    Conflict,
    /// The row was revoked or expired, or the user was deactivated.
    Gone,
}

/// A session [`record_logout`] revoked.
#[derive(Debug, Clone)]
pub struct RevokedSession {
    pub user_id: UserId,
    pub session_ref: Vec<u8>,
    pub sealed_provider_session: OpaqueSecret,
}

/// Request-scoped fields every audit row can carry.
#[derive(Debug, Clone, Copy, Default)]
pub struct AuditRequest<'a> {
    /// The correlation ID, or the inbox receipt ID for webhook-driven changes.
    pub request_id: Option<&'a str>,
    /// Keyed hash of the provider session, from [`SessionRefKey::session_ref`].
    pub session_ref: Option<&'a [u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    Active {
        user: UserRecord,
        /// True when this call created the user.
        provisioned: bool,
    },
    /// The person authenticated, but their DeepRef account is deactivated.
    Deactivated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deactivation {
    Deactivated(UserId),
    AlreadyDeactivated(UserId),
    UnknownUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditEvent {
    Login,
    Logout,
    SessionRevoked(SessionRevocationReason),
    UserProvisioned,
    UserDeactivated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionRevocationReason {
    UserLogout,
    UserDeactivated,
}

impl AuditEvent {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Login => "auth.login",
            Self::Logout => "auth.logout",
            Self::SessionRevoked(_) => "auth.session_revoked",
            Self::UserProvisioned => "user.provisioned",
            Self::UserDeactivated => "user.deactivated",
        }
    }

    /// The closed metadata for each event. No caller-supplied text reaches it.
    fn metadata(self) -> Value {
        match self {
            Self::SessionRevoked(SessionRevocationReason::UserLogout) => {
                json!({ "reason": "user_logout" })
            }
            Self::SessionRevoked(SessionRevocationReason::UserDeactivated) => {
                json!({ "reason": "user_deactivated" })
            }
            Self::UserDeactivated => json!({ "source": "provider_event" }),
            Self::Login | Self::Logout | Self::UserProvisioned => json!({}),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AuditPrincipal {
    User(UserId),
    System,
}

/// Writes one administrative audit row. Every caller writes it in the same
/// transaction as the change it records; `workspace_id` and `project_id` are
/// the identifiers of the tenancy the change happened in (03a), and stay NULL
/// for the identity events of 0085.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_admin_audit(
    tx: &mut Transaction<'_, Postgres>,
    event_type: &str,
    principal: AuditPrincipal,
    target: Option<UserId>,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    metadata: Value,
    audit: AuditRequest<'_>,
) -> Result<(), sqlx::Error> {
    let (kind, principal_user_id) = match principal {
        AuditPrincipal::User(id) => ("user", Some(id.as_uuid())),
        AuditPrincipal::System => ("system", None),
    };
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id,
            workspace_id, project_id, session_ref, request_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(kind)
    .bind(principal_user_id)
    .bind(target.map(UserId::as_uuid))
    .bind(workspace_id)
    .bind(project_id)
    .bind(audit.session_ref)
    .bind(audit.request_id)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The fixed user supplied by the API only when authentication is disabled in
/// development. The HTTP session gate and startup provisioning share this ID.
pub fn development_principal_id() -> UserId {
    UserId::new(Uuid::from_u128(3))
}

/// Ensure the fixed auth-off development principal exists and is active. The
/// caller passes the same runtime gate used by the session middleware; false
/// performs no database access and provisions nothing.
pub async fn ensure_development_principal(
    pool: &PgPool,
    enabled: bool,
) -> Result<Option<UserId>, IdentityStoreError> {
    if !enabled {
        return Ok(None);
    }

    const WORKOS_ID: &str = "deepref-development-principal";
    const EMAIL: &str = "development@deepref.invalid";
    let user_id = development_principal_id();
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users(id,workos_user_id,primary_email,display_name,status)
         VALUES($1,$2,$3,'Local development principal','active')
         ON CONFLICT(id) DO NOTHING RETURNING id",
    )
    .bind(user_id.as_uuid())
    .bind(WORKOS_ID)
    .bind(EMAIL)
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_some() {
        insert_admin_audit(
            &mut tx,
            "user.provisioned",
            AuditPrincipal::System,
            Some(user_id),
            None,
            None,
            json!({}),
            AuditRequest {
                session_ref: None,
                request_id: None,
            },
        )
        .await?;
    } else {
        let existing_external_id: Option<String> =
            sqlx::query_scalar("SELECT workos_user_id FROM users WHERE id=$1 FOR UPDATE")
                .bind(user_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await?;
        if existing_external_id.as_deref() != Some(WORKOS_ID) {
            return Err(IdentityStoreError::Corrupt(
                "development principal identity",
            ));
        }
        sqlx::query("UPDATE users SET status='active',updated_at=now() WHERE id=$1")
            .bind(user_id.as_uuid())
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(Some(user_id))
}

async fn write_audit(
    tx: &mut Transaction<'_, Postgres>,
    event: AuditEvent,
    principal: AuditPrincipal,
    target: Option<UserId>,
    audit: AuditRequest<'_>,
) -> Result<(), sqlx::Error> {
    insert_admin_audit(
        tx,
        event.as_str(),
        principal,
        target,
        None,
        None,
        event.metadata(),
        audit,
    )
    .await
}

const SELECT_BY_EXTERNAL_ID: &str = "SELECT id, workos_user_id, primary_email, display_name, status
     FROM users WHERE workos_user_id = $1";
const SELECT_BY_ID: &str = "SELECT id, workos_user_id, primary_email, display_name, status
     FROM users WHERE id = $1";

fn user_record(row: &PgRow) -> Result<UserRecord, IdentityStoreError> {
    let status: String = row.try_get("status")?;
    let external: String = row.try_get("workos_user_id")?;
    Ok(UserRecord {
        id: UserId::new(row.try_get("id")?),
        external_user_id: ExternalUserId::parse(external)
            .map_err(|_| IdentityStoreError::Corrupt("workos_user_id"))?,
        email: row.try_get("primary_email")?,
        display_name: row.try_get("display_name")?,
        status: UserStatus::parse(&status).ok_or(IdentityStoreError::Corrupt("status"))?,
    })
}

/// The address to record as verified: the provider's email, normalized, only
/// when the provider says it verified it.
fn verified_email(profile: &ProviderProfile) -> Option<String> {
    if profile.email_verified {
        deepref_application::normalize_invitation_email(&profile.email)
    } else {
        None
    }
}

/// Resolves the provider identity to its one DeepRef user, creating it on first
/// sign-in, and records the sign-in. Replaying the same callback creates no
/// second user and no second `user.provisioned` row.
pub async fn sign_in(
    pool: &PgPool,
    session: &ProviderSession,
    profile: &ProviderProfile,
    browser_session: NewBrowserSession<'_>,
    audit: AuditRequest<'_>,
) -> Result<SignIn, IdentityStoreError> {
    let mut tx = pool.begin().await?;
    let new_id = UserId::new(Uuid::new_v4());
    let inserted = sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, verified_email, display_name, status)
         VALUES ($1, $2, $3, $4, $5, 'active')
         ON CONFLICT (workos_user_id) DO NOTHING",
    )
    .bind(new_id.as_uuid())
    .bind(session.external_user_id.as_str())
    .bind(&profile.email)
    .bind(verified_email(profile))
    .bind(profile.display_name.as_deref())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if inserted {
        write_audit(
            &mut tx,
            AuditEvent::UserProvisioned,
            AuditPrincipal::User(new_id),
            Some(new_id),
            audit,
        )
        .await?;
    } else {
        // Profile fields follow the provider; `status` is never touched here,
        // so a profile update cannot reactivate a deactivated user. An account
        // whose deletion has begun or been fulfilled never re-absorbs provider PII
        // (11, BUSINESS_DECISIONS): the anonymized form stands. The durable
        // `users.deleted_at` marker is the version-independent guard (the
        // trigger of migration 0111 enforces it at the database boundary); the
        // fulfillment log is the belt-and-braces one that also covers a row
        // restored without its marker.
        sqlx::query(
            "UPDATE users
             SET primary_email = $2, verified_email = $4, display_name = $3, updated_at = now()
             WHERE workos_user_id = $1
               AND status = 'active'
               AND deleted_at IS NULL
               AND NOT EXISTS (
                 SELECT 1 FROM fulfilled_account_deletions f WHERE f.user_id = users.id)
               AND (primary_email IS DISTINCT FROM $2
                    OR display_name IS DISTINCT FROM $3
                    OR verified_email IS DISTINCT FROM $4)",
        )
        .bind(session.external_user_id.as_str())
        .bind(&profile.email)
        .bind(profile.display_name.as_deref())
        .bind(verified_email(profile))
        .execute(&mut *tx)
        .await?;
    }
    let row = sqlx::query(SELECT_BY_EXTERNAL_ID)
        .bind(session.external_user_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
    let user = user_record(&row)?;
    if user.status != UserStatus::Active {
        tx.commit().await?;
        return Ok(SignIn::Deactivated);
    }
    // Onboarding (03a) is tracked by `users.onboarded_at`, not by whether this
    // call inserted the row: a user an N-1 binary provisioned has no
    // memberships and is onboarded by their first sign-in here. The row lock
    // makes it exactly-once under concurrent callbacks and idempotent on retry.
    let onboarded_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT onboarded_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(user.id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
    if onboarded_at.is_none() {
        crate::workspaces::onboard_user(
            &mut tx,
            user.id,
            profile.display_name.as_deref(),
            &profile.email,
            audit,
        )
        .await?;
        sqlx::query("UPDATE users SET onboarded_at = now() WHERE id = $1")
            .bind(user.id.as_uuid())
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO user_sessions
           (id, user_id, session_token_hash, sealed_provider_session, session_ref, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(user.id.as_uuid())
    .bind(browser_session.token_hash)
    .bind(browser_session.sealed_provider_session.expose())
    .bind(browser_session.session_ref)
    .bind(browser_session.expires_at)
    .execute(&mut *tx)
    .await?;
    write_audit(
        &mut tx,
        AuditEvent::Login,
        AuditPrincipal::User(user.id),
        Some(user.id),
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(SignIn::Active {
        user,
        provisioned: inserted,
    })
}

/// Resolves an opaque cookie digest to an unrevoked, unexpired local session
/// and an active user. This query reads `users.status` on every request.
pub async fn find_active_session(
    pool: &PgPool,
    token_hash: &[u8],
) -> Result<Option<StoredBrowserSession>, IdentityStoreError> {
    let row = sqlx::query(
        "SELECT s.id AS session_id, s.sealed_provider_session, s.session_ref, s.version,
                u.id, u.workos_user_id, u.primary_email, u.display_name, u.status
         FROM user_sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.session_token_hash = $1
           AND s.revoked_at IS NULL
           AND s.expires_at > now()
           AND u.status = 'active'",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(StoredBrowserSession {
            id: row.try_get("session_id")?,
            user: user_record(&row)?,
            sealed_provider_session: OpaqueSecret::new(
                row.try_get::<String, _>("sealed_provider_session")?,
            ),
            session_ref: row.try_get("session_ref")?,
            version: row.try_get("version")?,
        })
    })
    .transpose()
}

/// What [`claim_refresh_lease`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshLease {
    /// The caller now alone may spend this session's refresh token.
    Claimed,
    /// Another request holds an unexpired lease; its commit is still pending.
    Held,
    /// The row moved past `expected_version`: someone already refreshed it.
    Superseded,
    /// The row was revoked or expired, or the user was deactivated.
    Gone,
}

/// Takes the refresh lease on a session for `ttl`, but only while the row is
/// still at `expected_version` and no other unexpired lease exists. The
/// provider spends a refresh token once, so exactly one request across all
/// replicas may call it; the rest wait for [`update_active_session`].
pub async fn claim_refresh_lease(
    pool: &PgPool,
    session_id: Uuid,
    expected_version: i64,
    owner: Uuid,
    ttl: std::time::Duration,
) -> Result<RefreshLease, IdentityStoreError> {
    let claimed = sqlx::query(
        "UPDATE user_sessions s
         SET refresh_lease_owner = $3,
             refresh_lease_until = now() + make_interval(secs => $4)
         FROM users u
         WHERE s.id = $1
           AND s.version = $2
           AND (s.refresh_lease_until IS NULL OR s.refresh_lease_until <= now())
           AND u.id = s.user_id
           AND u.status = 'active'
           AND s.revoked_at IS NULL
           AND s.expires_at > now()",
    )
    .bind(session_id)
    .bind(expected_version)
    .bind(owner)
    .bind(ttl.as_secs_f64())
    .execute(pool)
    .await?
    .rows_affected();
    if claimed == 1 {
        return Ok(RefreshLease::Claimed);
    }
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT s.version FROM user_sessions s JOIN users u ON u.id = s.user_id
         WHERE s.id = $1 AND u.status = 'active'
           AND s.revoked_at IS NULL AND s.expires_at > now()",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(match current {
        None => RefreshLease::Gone,
        Some(version) if version != expected_version => RefreshLease::Superseded,
        Some(_) => RefreshLease::Held,
    })
}

/// Gives the lease back after a refresh that persisted nothing, so the next
/// request need not wait for it to expire. A lease another request has since
/// taken over is left alone.
pub async fn release_refresh_lease(
    pool: &PgPool,
    session_id: Uuid,
    owner: Uuid,
) -> Result<(), IdentityStoreError> {
    sqlx::query(
        "UPDATE user_sessions
         SET refresh_lease_owner = NULL, refresh_lease_until = NULL
         WHERE id = $1 AND refresh_lease_owner = $2",
    )
    .bind(session_id)
    .bind(owner)
    .execute(pool)
    .await?;
    Ok(())
}

/// Ends the session because the provider rejected its refresh token, but only
/// if `lease_owner` still holds the refresh lease and the row is still at
/// `expected_version`. The check and the revocation are one statement, so a
/// request whose lease lapsed (and whose rejection may be a competitor's
/// replay of a token the real owner already rotated) cannot end a session that
/// is being, or has been, renewed. Returns whether the session was ended; only
/// then may the caller treat the session as over.
pub async fn end_session_if_leased(
    pool: &PgPool,
    session_id: Uuid,
    expected_version: i64,
    lease_owner: Uuid,
) -> Result<bool, IdentityStoreError> {
    let ended = sqlx::query(
        "UPDATE user_sessions
         SET revoked_at = now(), refresh_lease_owner = NULL, refresh_lease_until = NULL
         WHERE id = $1
           AND version = $2
           AND refresh_lease_owner = $3
           AND revoked_at IS NULL",
    )
    .bind(session_id)
    .bind(expected_version)
    .bind(lease_owner)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(ended == 1)
}

/// Persists a WorkOS refresh in the existing local session row, but only if the
/// row is still at `expected_version` and `lease_owner` still holds the refresh
/// lease. A request that lost either race holds a session that is already
/// superseded: it gets [`SessionUpdate::Conflict`], changes nothing, and must
/// reload the winner's session. Committing releases the lease.
pub async fn update_active_session(
    pool: &PgPool,
    session_id: Uuid,
    expected_version: i64,
    lease_owner: Uuid,
    sealed_provider_session: &OpaqueSecret,
    session_ref: &[u8],
) -> Result<SessionUpdate, IdentityStoreError> {
    let changed = sqlx::query(
        "UPDATE user_sessions s
         SET sealed_provider_session = $4, session_ref = $5, version = s.version + 1,
             refresh_lease_owner = NULL, refresh_lease_until = NULL
         FROM users u
         WHERE s.id = $1
           AND s.version = $2
           AND s.refresh_lease_owner = $3
           AND u.id = s.user_id
           AND u.status = 'active'
           AND s.revoked_at IS NULL
           AND s.expires_at > now()",
    )
    .bind(session_id)
    .bind(expected_version)
    .bind(lease_owner)
    .bind(sealed_provider_session.expose())
    .bind(session_ref)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 1 {
        return Ok(SessionUpdate::Updated);
    }
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM user_sessions s JOIN users u ON u.id = s.user_id
           WHERE s.id = $1 AND u.status = 'active'
             AND s.revoked_at IS NULL AND s.expires_at > now())",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    Ok(if live {
        SessionUpdate::Conflict
    } else {
        SessionUpdate::Gone
    })
}

/// The per-request session check: reads the user row with no cache in front.
pub async fn find_user_by_external_id(
    pool: &PgPool,
    external_user_id: &ExternalUserId,
) -> Result<Option<UserRecord>, IdentityStoreError> {
    sqlx::query(SELECT_BY_EXTERNAL_ID)
        .bind(external_user_id.as_str())
        .fetch_optional(pool)
        .await?
        .as_ref()
        .map(user_record)
        .transpose()
}

pub async fn find_user(
    pool: &PgPool,
    user_id: UserId,
) -> Result<Option<UserRecord>, IdentityStoreError> {
    sqlx::query(SELECT_BY_ID)
        .bind(user_id.as_uuid())
        .fetch_optional(pool)
        .await?
        .as_ref()
        .map(user_record)
        .transpose()
}

/// Revokes the local session named by the cookie digest and audits
/// `auth.logout` and `auth.session_revoked`. It needs nothing from the identity
/// provider, so a provider outage cannot leave a signed-out session usable.
/// Returns `None` when no unrevoked row matches (already signed out, unknown).
pub async fn record_logout(
    pool: &PgPool,
    token_hash: &[u8],
    audit: AuditRequest<'_>,
) -> Result<Option<RevokedSession>, IdentityStoreError> {
    let mut tx = pool.begin().await?;
    // Finalization locks the user before deleting that user's sessions. Take
    // the same order here so logout cannot hold a session row while waiting on
    // the user's foreign-key lock from its audit insert.
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM user_sessions
         WHERE session_token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(user_id) = user_id else {
        tx.commit().await?;
        return Ok(None);
    };
    let locked_user: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if locked_user.is_none() {
        tx.commit().await?;
        return Ok(None);
    }
    let row = sqlx::query(
        "UPDATE user_sessions
         SET revoked_at = now()
         WHERE session_token_hash = $1 AND user_id = $2 AND revoked_at IS NULL
         RETURNING user_id, session_ref, sealed_provider_session",
    )
    .bind(token_hash)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let revoked = RevokedSession {
        user_id: UserId::new(row.try_get("user_id")?),
        session_ref: row.try_get("session_ref")?,
        sealed_provider_session: OpaqueSecret::new(
            row.try_get::<String, _>("sealed_provider_session")?,
        ),
    };
    let audit = AuditRequest {
        request_id: audit.request_id,
        session_ref: Some(&revoked.session_ref),
    };
    write_audit(
        &mut tx,
        AuditEvent::Logout,
        AuditPrincipal::User(revoked.user_id),
        Some(revoked.user_id),
        audit,
    )
    .await?;
    write_audit(
        &mut tx,
        AuditEvent::SessionRevoked(SessionRevocationReason::UserLogout),
        AuditPrincipal::User(revoked.user_id),
        Some(revoked.user_id),
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(revoked))
}

/// Ends the DeepRef user in every workspace: compare-and-set from `active` to
/// `deactivated`, with `user.deactivated` written in the same transaction. A
/// replay changes and records nothing. No path here reactivates a user, and
/// `deleted_at`, the email and the display name are left alone.
pub async fn deactivate_user(
    pool: &PgPool,
    external_user_id: &ExternalUserId,
    request_id: &str,
) -> Result<Deactivation, IdentityStoreError> {
    let mut tx = pool.begin().await?;
    let changed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE users SET status = 'deactivated', updated_at = now()
         WHERE workos_user_id = $1 AND status = 'active'
         RETURNING id",
    )
    .bind(external_user_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = changed {
        let user_id = UserId::new(id);
        let session_refs: Vec<Vec<u8>> = sqlx::query_scalar(
            "UPDATE user_sessions
             SET revoked_at = now()
             WHERE user_id = $1 AND revoked_at IS NULL
             RETURNING session_ref",
        )
        .bind(user_id.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        for session_ref in session_refs {
            write_audit(
                &mut tx,
                AuditEvent::SessionRevoked(SessionRevocationReason::UserDeactivated),
                AuditPrincipal::System,
                Some(user_id),
                AuditRequest {
                    request_id: Some(request_id),
                    session_ref: Some(&session_ref),
                },
            )
            .await?;
        }
        write_audit(
            &mut tx,
            AuditEvent::UserDeactivated,
            AuditPrincipal::System,
            Some(user_id),
            AuditRequest {
                request_id: Some(request_id),
                session_ref: None,
            },
        )
        .await?;
        tx.commit().await?;
        return Ok(Deactivation::Deactivated(user_id));
    }
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = $1")
            .bind(external_user_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(existing.map_or(Deactivation::UnknownUser, |id| {
        Deactivation::AlreadyDeactivated(UserId::new(id))
    }))
}

/// Asks the provider to end every session of a deactivated user. Best effort:
/// requests are already rejected by the status check, so a provider failure is
/// logged and never surfaces. Returns how many sessions were revoked.
pub async fn revoke_deactivated_user_sessions(
    provider: &dyn IdentityProvider,
    external_user_id: &ExternalUserId,
) -> usize {
    let session_ids = match provider.list_session_ids(external_user_id).await {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(%error, "could not list sessions to revoke");
            return 0;
        }
    };
    let mut revoked = 0;
    for session_id in session_ids {
        if let Err(error) = provider.revoke_session(&session_id).await {
            tracing::warn!(%error, "could not revoke a provider session");
            continue;
        }
        revoked += 1;
    }
    revoked
}

/// The seam the 04a integration inbox calls for a verified WorkOS `user.deleted`
/// delivery. `receipt_id` is the inbox receipt, stored as the audit `request_id`.
/// The inbox owns signature verification and provider-event deduplication; this
/// function is itself replay-safe.
pub async fn apply_provider_user_deleted(
    pool: &PgPool,
    provider: &dyn IdentityProvider,
    external_user_id: &ExternalUserId,
    receipt_id: &str,
) -> Result<Deactivation, IdentityStoreError> {
    let outcome = deactivate_user(pool, external_user_id, receipt_id).await?;
    if matches!(outcome, Deactivation::Deactivated(_)) {
        revoke_deactivated_user_sessions(provider, external_user_id).await;
    }
    Ok(outcome)
}
