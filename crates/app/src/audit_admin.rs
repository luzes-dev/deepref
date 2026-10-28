//! Incident-response query and retention purge over `admin_audit_events`
//! (handoff 11 Part B).
//!
//! The administrative audit is append-only and separately purged on the
//! BUSINESS_DECISIONS retention: 6 months for administrative and security
//! rows, 5 years without content for the record that a deletion request was
//! fulfilled (`account.deletion_completed` and `fulfilled_account_deletions`).
//! The restricted retention function (0188) is the maintenance path. The
//! append-only trigger checks its trusted owner and each row's cutoff.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

/// Retention of administrative and security audit rows (BUSINESS_DECISIONS).
pub const ADMIN_AUDIT_RETENTION: &str = "6 months";

/// Retention of the proof that a deletion request was fulfilled: five years,
/// without content (BUSINESS_DECISIONS).
pub const DELETION_PROOF_RETENTION: &str = "5 years";

/// Retention of session rows (BUSINESS_DECISIONS): 30 days after the session
/// expired or was revoked. Before that a row may still be needed to explain a
/// request; after it the sealed provider payload has no purpose and goes.
pub const SESSION_RETENTION: &str = "30 days";

/// Dry-run count follows the trigger/function retention policy. Actual writes
/// use the privileged bounded function; SQLx macros are not involved.
const PURGEABLE_COUNT_SQL: &str = "SELECT count(*) FROM public.admin_audit_events
   WHERE (event_type <> 'account.deletion_completed'
          AND occurred_at < now() - interval '6 months')
      OR (event_type = 'account.deletion_completed'
          AND occurred_at < now() - interval '5 years')";

/// How many rows one purge statement removes. Bounded so the delete never
/// holds a long lock or floods WAL in one transaction.
const DEFAULT_PURGE_BATCH: i64 = 1_000;

/// Incident-response filters, all optional and combined with AND.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub workspace_id: Option<Uuid>,
    pub principal_user_id: Option<Uuid>,
    pub target_user_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Maximum rows returned; clamped to `1..=10_000`. Zero means the default.
    pub limit: i64,
}

/// One administrative audit row, as `deepref-server audit query` prints it.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEventRow {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub principal_kind: String,
    pub principal_user_id: Option<Uuid>,
    pub target_user_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub request_id: Option<String>,
    pub metadata: Value,
}

fn event_row(row: &sqlx::postgres::PgRow) -> Result<AuditEventRow, sqlx::Error> {
    Ok(AuditEventRow {
        id: row.try_get("id")?,
        occurred_at: row.try_get("occurred_at")?,
        event_type: row.try_get("event_type")?,
        principal_kind: row.try_get("principal_kind")?,
        principal_user_id: row.try_get("principal_user_id")?,
        target_user_id: row.try_get("target_user_id")?,
        workspace_id: row.try_get("workspace_id")?,
        project_id: row.try_get("project_id")?,
        request_id: row.try_get("request_id")?,
        metadata: row.try_get("metadata")?,
    })
}

/// Queries the administrative audit by workspace, principal, target, event
/// type and time range, newest first. The composite indexes added in
/// migrations 0114-0115 (with the principal and target indexes of 0085) keep
/// every filter indexed.
pub async fn query_admin_audit(
    pool: &PgPool,
    query: &AuditQuery,
) -> Result<Vec<AuditEventRow>, sqlx::Error> {
    let limit = if query.limit <= 0 {
        DEFAULT_PURGE_BATCH
    } else {
        query.limit.clamp(1, 10_000)
    };
    let mut sql = QueryBuilder::<Postgres>::new(
        "SELECT id, occurred_at, event_type, principal_kind, principal_user_id, target_user_id,
                workspace_id, project_id, request_id, metadata
         FROM public.admin_audit_events WHERE ",
    );
    let mut first = true;
    macro_rules! condition {
        ($clause:expr) => {
            if first {
                first = false;
            } else {
                sql.push(" AND ");
            }
            sql.push($clause);
        };
    }
    if let Some(workspace_id) = query.workspace_id {
        condition!("workspace_id = ");
        sql.push_bind(workspace_id);
    }
    if let Some(principal_user_id) = query.principal_user_id {
        condition!("principal_user_id = ");
        sql.push_bind(principal_user_id);
    }
    if let Some(target_user_id) = query.target_user_id {
        condition!("target_user_id = ");
        sql.push_bind(target_user_id);
    }
    if let Some(event_type) = query.event_type.as_deref() {
        condition!("event_type = ");
        sql.push_bind(event_type);
    }
    if let Some(since) = query.since {
        condition!("occurred_at >= ");
        sql.push_bind(since);
    }
    if let Some(until) = query.until {
        condition!("occurred_at <= ");
        sql.push_bind(until);
    }
    if first {
        sql.push("TRUE");
    }
    sql.push(" ORDER BY occurred_at DESC, id DESC LIMIT ");
    sql.push_bind(limit);
    let rows = sql.build().fetch_all(pool).await?;
    rows.iter().map(event_row).collect()
}

/// What one `deepref-server audit purge` run did (or would do).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgeReport {
    pub dry_run: bool,
    pub batch_size: i64,
    /// Bounded audit-delete batches; the other two rows kinds go in one
    /// statement each and are not counted here.
    pub batches: u32,
    pub audit_rows_purged: u64,
    pub fulfilled_deletion_rows_purged: u64,
    /// Session rows 30 days past expiry or revocation (BUSINESS_DECISIONS).
    pub session_rows_purged: u64,
}

/// Deletes audit rows past their retention cutoff, in bounded batches, and
/// reports what it removed. With `dry_run` nothing is deleted; the report
/// counts what a real run would remove.
///
/// Executes the restricted SECURITY DEFINER retention function. One call has
/// at most 32 audit batches plus one batch each of sessions/fulfilled proofs;
/// deletion and audit summary commit atomically. Repeat calls handle remaining
/// eligible rows. A runtime credential cannot invoke this maintenance path.
pub async fn purge_admin_audit(
    pool: &PgPool,
    dry_run: bool,
    batch_size: Option<i64>,
) -> Result<PurgeReport, sqlx::Error> {
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        purge_admin_audit_inner(pool, dry_run, batch_size),
    )
    .await
    .map_err(|_| {
        sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "audit retention deadline exceeded; inspect committed audit evidence before repeating",
        ))
    })?
}

async fn purge_admin_audit_inner(
    pool: &PgPool,
    dry_run: bool,
    batch_size: Option<i64>,
) -> Result<PurgeReport, sqlx::Error> {
    let batch_size = batch_size.unwrap_or(DEFAULT_PURGE_BATCH).clamp(1, 1_000);
    let mut report = PurgeReport {
        dry_run,
        batch_size,
        batches: 0,
        audit_rows_purged: 0,
        fulfilled_deletion_rows_purged: 0,
        session_rows_purged: 0,
    };
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout='3s'")
        .execute(&mut *tx)
        .await?;
    if dry_run {
        let pending: i64 = sqlx::query_scalar(PURGEABLE_COUNT_SQL)
            .fetch_one(&mut *tx)
            .await?;
        report.audit_rows_purged = u64::try_from(pending).unwrap_or(0);
        report.fulfilled_deletion_rows_purged = count_purgeable_fulfilled_rows(&mut tx).await?;
        report.session_rows_purged = count_purgeable_sessions(&mut tx).await?;
        tx.commit().await?;
        return Ok(report);
    }
    let row=sqlx::query("SELECT batches,audit_rows,fulfilled_rows,session_rows FROM public.purge_retention_records($1)")
        .bind(i32::try_from(batch_size).unwrap_or(1000)).fetch_one(&mut *tx).await?;
    report.batches = u32::try_from(row.try_get::<i32, _>("batches")?).unwrap_or(0);
    report.audit_rows_purged = u64::try_from(row.try_get::<i64, _>("audit_rows")?).unwrap_or(0);
    report.fulfilled_deletion_rows_purged =
        u64::try_from(row.try_get::<i64, _>("fulfilled_rows")?).unwrap_or(0);
    report.session_rows_purged = u64::try_from(row.try_get::<i64, _>("session_rows")?).unwrap_or(0);
    tx.commit().await?;
    Ok(report)
}

/// The fulfilled-deletion log contains pseudonymous address HMACs but no
/// plaintext; those values are also removed after five years (BUSINESS_DECISIONS).
async fn count_purgeable_fulfilled_rows(
    connection: &mut sqlx::PgConnection,
) -> Result<u64, sqlx::Error> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.fulfilled_account_deletions
         WHERE completed_at < now() - interval '5 years'",
    )
    .fetch_one(connection)
    .await?;
    Ok(u64::try_from(pending).unwrap_or(0))
}

/// Session rows are retained for 30 days after they expired or were revoked
/// (BUSINESS_DECISIONS), then removed in bounded batches with everything else
/// in the same daily run. A session row is the only place a sealed provider
/// payload is kept, so this is also the retention that bounds that data.
const PURGEABLE_SESSIONS_COUNT_SQL: &str = "SELECT count(*) FROM public.user_sessions
   WHERE (revoked_at IS NOT NULL AND revoked_at < now() - interval '30 days')
      OR (expires_at < now() - interval '30 days')";

async fn count_purgeable_sessions(connection: &mut sqlx::PgConnection) -> Result<u64, sqlx::Error> {
    let pending: i64 = sqlx::query_scalar(PURGEABLE_SESSIONS_COUNT_SQL)
        .fetch_one(connection)
        .await?;
    Ok(u64::try_from(pending).unwrap_or(0))
}

/// Read-only diagnostic after a purge commits. A separate bounded transaction
/// uses one borrowed connection; no runtime GUC or write authority is needed.
pub async fn remaining_retention_eligible(pool: &PgPool) -> Result<bool, sqlx::Error> {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut transaction=crate::operational_read::begin(pool).await?;
        let remaining=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.admin_audit_events WHERE occurred_at<now()-CASE WHEN event_type='account.deletion_completed' THEN interval '5 years' ELSE interval '6 months' END) OR EXISTS(SELECT 1 FROM public.fulfilled_account_deletions WHERE completed_at<now()-interval '5 years') OR EXISTS(SELECT 1 FROM public.user_sessions WHERE revoked_at<now()-interval '30 days' OR expires_at<now()-interval '30 days')")
            .fetch_one(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(remaining)
    }).await.map_err(|_| sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut,
        "retention remaining-row diagnostic deadline exceeded; inspect committed purge evidence")))?
}
