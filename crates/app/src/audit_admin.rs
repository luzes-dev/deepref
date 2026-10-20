//! Incident-response query and retention purge over `admin_audit_events`
//! (handoff 11 Part B).
//!
//! The administrative audit is append-only and separately purged on the
//! BUSINESS_DECISIONS retention: 6 months for administrative and security
//! rows, 5 years without content for the record that a deletion request was
//! fulfilled (`account.deletion_completed` and `fulfilled_account_deletions`).
//! The purge is the only delete path the append-only trigger (migration
//! 0112) ever lets through, and only for rows already past their cutoff.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};

/// Retention of administrative and security audit rows (BUSINESS_DECISIONS).
pub const ADMIN_AUDIT_RETENTION: &str = "6 months";

/// Retention of the proof that a deletion request was fulfilled: five years,
/// without content (BUSINESS_DECISIONS).
pub const DELETION_PROOF_RETENTION: &str = "5 years";

/// The rows the purge may remove, decided exactly as the trigger in migration
/// 0112 decides them. Stated twice (count and delete) because `sqlx` only
/// accepts literal SQL; the purge tests assert both agree with the trigger.
const PURGEABLE_COUNT_SQL: &str = "SELECT count(*) FROM admin_audit_events
   WHERE (event_type <> 'account.deletion_completed'
          AND occurred_at < now() - interval '6 months')
      OR (event_type = 'account.deletion_completed'
          AND occurred_at < now() - interval '5 years')";

const PURGEABLE_DELETE_SQL: &str = "DELETE FROM admin_audit_events
   WHERE id IN (
     SELECT id FROM admin_audit_events
     WHERE (event_type <> 'account.deletion_completed'
            AND occurred_at < now() - interval '6 months')
        OR (event_type = 'account.deletion_completed'
            AND occurred_at < now() - interval '5 years')
     ORDER BY occurred_at, id
     LIMIT $1)";

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
         FROM admin_audit_events WHERE ",
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
    pub batches: u32,
    pub audit_rows_purged: u64,
    pub fulfilled_deletion_rows_purged: u64,
}

/// Deletes audit rows past their retention cutoff, in bounded batches, and
/// reports what it removed. With `dry_run` nothing is deleted; the report
/// counts what a real run would remove.
///
/// Every deleting transaction runs with `SET LOCAL deepref.audit_purge = 'on'`,
/// the only key the append-only trigger (migration 0112) honours, and only
/// then for rows already past their cutoff. Rows inside retention cannot be
/// deleted by this function or any other.
pub async fn purge_admin_audit(
    pool: &PgPool,
    dry_run: bool,
    batch_size: Option<i64>,
) -> Result<PurgeReport, sqlx::Error> {
    let batch_size = batch_size.unwrap_or(DEFAULT_PURGE_BATCH).clamp(1, 10_000);
    let mut report = PurgeReport {
        dry_run,
        batch_size,
        batches: 0,
        audit_rows_purged: 0,
        fulfilled_deletion_rows_purged: 0,
    };
    if dry_run {
        let pending: i64 = sqlx::query_scalar(PURGEABLE_COUNT_SQL)
            .fetch_one(pool)
            .await?;
        report.audit_rows_purged = u64::try_from(pending).unwrap_or(0);
        report.fulfilled_deletion_rows_purged = count_purgeable_fulfilled_rows(pool).await?;
        return Ok(report);
    }
    loop {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL deepref.audit_purge = 'on'")
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query(PURGEABLE_DELETE_SQL)
            .bind(batch_size)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        report.batches += 1;
        report.audit_rows_purged += deleted;
        if deleted < batch_size as u64 {
            break;
        }
    }
    report.fulfilled_deletion_rows_purged = purge_fulfilled_rows(pool).await?;
    // Purging the audit is itself an administrative action (11 Part B): each
    // real run records that it ran and how many rows it removed. A dry run
    // changes nothing and records nothing.
    let mut tx = pool.begin().await?;
    insert_admin_audit(
        &mut tx,
        "audit.purged",
        AuditPrincipal::System,
        None,
        None,
        None,
        json!({ "rows_purged": report.audit_rows_purged }),
        AuditRequest::default(),
    )
    .await?;
    tx.commit().await?;
    Ok(report)
}

/// The fulfilled-deletion log is a compliance record with no content; after
/// five years (BUSINESS_DECISIONS) its rows go the same way.
async fn count_purgeable_fulfilled_rows(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fulfilled_account_deletions
         WHERE completed_at < now() - interval '5 years'",
    )
    .fetch_one(pool)
    .await?;
    Ok(u64::try_from(pending).unwrap_or(0))
}

async fn purge_fulfilled_rows(pool: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "DELETE FROM fulfilled_account_deletions
         WHERE completed_at < now() - interval '5 years'",
    )
    .execute(pool)
    .await?
    .rows_affected())
}
