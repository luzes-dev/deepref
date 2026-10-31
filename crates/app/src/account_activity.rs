//! Durable evidence from explicit authenticated product activity.
//! Scientific actor attribution, worker work and session polling are not
//! evidence. HTTP policy decides which human command attempts call this port.

use deepref_domain::UserId;
use sqlx::PgPool;
use uuid::Uuid;

/// Persist a command attempt before its handler can commit business effects.
/// This receipt grants no permission and does not assert business success.
/// The caller supplies the actual authenticated local session, not a header.
pub async fn record_product_command_attempt(
    pool: &PgPool,
    user: UserId,
    session: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Match logout/deactivation's user-before-session order. NO KEY UPDATE is
    // compatible with the shared user FK locks taken by workspace writers.
    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1 AND status='active' AND deleted_at IS NULL
         AND NOT EXISTS(SELECT 1 FROM fulfilled_account_deletions f WHERE f.user_id=users.id)
         FOR NO KEY UPDATE",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if active.is_none() {
        tx.commit().await?;
        return Ok(false);
    }
    let live: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM user_sessions WHERE id=$1 AND user_id=$2
         AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE",
    )
    .bind(session)
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if live.is_none() {
        tx.commit().await?;
        return Ok(false);
    }
    // Recheck expiry at the actual write; a wait or timeout is never evidence.
    let recorded: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO account_human_activity(user_id,generation,occurred_at,evidence_kind)
         SELECT $1,$2,clock_timestamp(),'product_command'
         WHERE EXISTS(SELECT 1 FROM user_sessions WHERE id=$3 AND user_id=$1
           AND revoked_at IS NULL AND expires_at>clock_timestamp())
         ON CONFLICT(user_id) DO UPDATE SET generation=EXCLUDED.generation,
           occurred_at=EXCLUDED.occurred_at,evidence_kind=EXCLUDED.evidence_kind
         RETURNING user_id",
    )
    .bind(user.as_uuid())
    .bind(Uuid::new_v4())
    .bind(session)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(recorded.is_some())
}
