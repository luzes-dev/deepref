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
enum AuditPrincipal {
    User(UserId),
    System,
}

async fn write_audit(
    tx: &mut Transaction<'_, Postgres>,
    event: AuditEvent,
    principal: AuditPrincipal,
    target: Option<UserId>,
    audit: AuditRequest<'_>,
) -> Result<(), sqlx::Error> {
    let (kind, principal_user_id) = match principal {
        AuditPrincipal::User(id) => ("user", Some(id.as_uuid())),
        AuditPrincipal::System => ("system", None),
    };
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id,
            session_ref, request_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(event.as_str())
    .bind(kind)
    .bind(principal_user_id)
    .bind(target.map(UserId::as_uuid))
    .bind(audit.session_ref)
    .bind(audit.request_id)
    .bind(event.metadata())
    .execute(&mut **tx)
    .await?;
    Ok(())
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
        "INSERT INTO users (id, workos_user_id, primary_email, display_name, status)
         VALUES ($1, $2, $3, $4, 'active')
         ON CONFLICT (workos_user_id) DO NOTHING",
    )
    .bind(new_id.as_uuid())
    .bind(session.external_user_id.as_str())
    .bind(&profile.email)
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
        // so a profile update cannot reactivate a deactivated user.
        sqlx::query(
            "UPDATE users SET primary_email = $2, display_name = $3, updated_at = now()
             WHERE workos_user_id = $1
               AND (primary_email IS DISTINCT FROM $2 OR display_name IS DISTINCT FROM $3)",
        )
        .bind(session.external_user_id.as_str())
        .bind(&profile.email)
        .bind(profile.display_name.as_deref())
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
        "SELECT s.id AS session_id, s.sealed_provider_session, s.session_ref,
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
        })
    })
    .transpose()
}

/// Persists a WorkOS refresh in the existing local session row. Returning
/// `false` means that the row was revoked or the user was deactivated while
/// the provider refresh was in flight.
pub async fn update_active_session(
    pool: &PgPool,
    session_id: Uuid,
    sealed_provider_session: &OpaqueSecret,
    session_ref: &[u8],
) -> Result<bool, IdentityStoreError> {
    let changed = sqlx::query(
        "UPDATE user_sessions s
         SET sealed_provider_session = $2, session_ref = $3
         FROM users u
         WHERE s.id = $1
           AND u.id = s.user_id
           AND u.status = 'active'
           AND s.revoked_at IS NULL
           AND s.expires_at > now()",
    )
    .bind(session_id)
    .bind(sealed_provider_session.expose())
    .bind(session_ref)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(changed == 1)
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

pub async fn record_logout(
    pool: &PgPool,
    user_id: UserId,
    token_hash: &[u8],
    audit: AuditRequest<'_>,
) -> Result<(), IdentityStoreError> {
    let mut tx = pool.begin().await?;
    let session_ref: Option<Vec<u8>> = sqlx::query_scalar(
        "UPDATE user_sessions
         SET revoked_at = now()
         WHERE user_id = $1 AND session_token_hash = $2 AND revoked_at IS NULL
         RETURNING session_ref",
    )
    .bind(user_id.as_uuid())
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(session_ref) = session_ref {
        let audit = AuditRequest {
            request_id: audit.request_id,
            session_ref: Some(&session_ref),
        };
        write_audit(
            &mut tx,
            AuditEvent::Logout,
            AuditPrincipal::User(user_id),
            Some(user_id),
            audit,
        )
        .await?;
        write_audit(
            &mut tx,
            AuditEvent::SessionRevoked(SessionRevocationReason::UserLogout),
            AuditPrincipal::User(user_id),
            Some(user_id),
            audit,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
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
