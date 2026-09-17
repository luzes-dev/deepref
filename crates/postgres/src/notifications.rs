use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

/// Bound the inbox so the newest window always stays cheap to read. Read rows
/// older than the retention window are pruned first, then the table is capped
/// by absolute size.
const RETENTION_DAYS_READ: &str = "30 days";
const RETENTION_BATCH: i64 = 500;
const MAX_ROWS: i64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationSeverity {
    Success,
    Info,
    Warning,
    Error,
}

impl NotificationSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A human-readable record of a server-side async outcome. Emitters create the
/// draft inside the same transaction that made the underlying transition
/// authoritative, so a notification commits or rolls back with its event.
#[derive(Debug, Clone)]
pub struct NotificationDraft {
    pub kind: String,
    pub severity: NotificationSeverity,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub body: Option<String>,
    pub payload: serde_json::Value,
}

impl NotificationDraft {
    pub fn success(
        kind: &str,
        project_id: Option<Uuid>,
        title: &str,
        body: Option<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: NotificationSeverity::Success,
            project_id,
            title: title.to_owned(),
            body,
            payload,
        }
    }

    pub fn info(
        kind: &str,
        project_id: Option<Uuid>,
        title: &str,
        body: Option<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: NotificationSeverity::Info,
            project_id,
            title: title.to_owned(),
            body,
            payload,
        }
    }

    pub fn warning(
        kind: &str,
        project_id: Option<Uuid>,
        title: &str,
        body: Option<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: NotificationSeverity::Warning,
            project_id,
            title: title.to_owned(),
            body,
            payload,
        }
    }

    pub fn error(
        kind: &str,
        project_id: Option<Uuid>,
        title: &str,
        body: Option<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: NotificationSeverity::Error,
            project_id,
            title: title.to_owned(),
            body,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct NotificationRecord {
    pub id: Uuid,
    pub revision: i64,
    pub kind: String,
    pub severity: String,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub body: Option<String>,
    pub payload: serde_json::Value,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub struct NotificationPage {
    pub items: Vec<NotificationRecord>,
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationUnreadSummary {
    pub count: i64,
    pub latest_revision: i64,
}

/// Record a notification inside an existing transaction so it commits or rolls
/// back together with the state change it describes.
pub async fn record_notification_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    draft: &NotificationDraft,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO notifications (kind,severity,project_id,title,body,payload)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(&draft.kind)
    .bind(draft.severity.as_str())
    .bind(draft.project_id)
    .bind(&draft.title)
    .bind(draft.body.as_deref().filter(|body| !body.trim().is_empty()))
    .bind(&draft.payload)
    .execute(&mut **tx)
    .await?;
    prune_in_transaction(tx).await
}

/// Record a notification on a standalone connection, for emitters that do not
/// hold a transaction.
pub async fn record_notification(
    pool: &PgPool,
    draft: &NotificationDraft,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    record_notification_in_transaction(&mut tx, draft).await?;
    tx.commit().await
}

async fn prune_in_transaction(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM notifications
         WHERE id IN (
           SELECT id FROM notifications
           WHERE read_at IS NOT NULL AND created_at < now() - $1::interval
           LIMIT $2
         )",
    )
    .bind(RETENTION_DAYS_READ)
    .bind(RETENTION_BATCH)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM notifications
         WHERE id IN (
           SELECT id FROM notifications
           ORDER BY revision DESC
           OFFSET $1
         )",
    )
    .bind(MAX_ROWS)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Newest-first bounded page keyed by the monotonic `revision` sequence.
pub async fn list_notifications(
    pool: &PgPool,
    before_revision: Option<i64>,
    limit: i64,
) -> Result<NotificationPage, sqlx::Error> {
    let rows = sqlx::query_as::<_, NotificationRecord>(
        "SELECT id,revision,kind,severity,project_id,title,body,payload,read_at,created_at
         FROM notifications
         WHERE ($1::bigint IS NULL OR revision < $1)
         ORDER BY revision DESC
         LIMIT $2",
    )
    .bind(before_revision)
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;
    let mut items: Vec<NotificationRecord> = rows.into_iter().collect();
    let next_cursor = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| item.revision)
    } else {
        None
    };
    Ok(NotificationPage { items, next_cursor })
}

pub async fn unread_summary(pool: &PgPool) -> Result<NotificationUnreadSummary, sqlx::Error> {
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE read_at IS NULL)::bigint AS unread,
                COALESCE(max(revision),0)::bigint AS latest_revision
         FROM notifications",
    )
    .fetch_one(pool)
    .await?;
    Ok(NotificationUnreadSummary {
        count: row.get("unread"),
        latest_revision: row.get("latest_revision"),
    })
}

/// Mark notifications read by explicit ids, or everything when `all` is set.
/// Returns the number of rows that actually transitioned to read.
#[derive(Debug, Clone, Default)]
pub struct MarkNotificationsRead {
    pub ids: Option<Vec<Uuid>>,
    pub all: bool,
}

pub async fn mark_notifications_read(
    pool: &PgPool,
    request: &MarkNotificationsRead,
) -> Result<i64, sqlx::Error> {
    let result = if request.all {
        sqlx::query("UPDATE notifications SET read_at=now() WHERE read_at IS NULL")
            .execute(pool)
            .await?
    } else {
        match &request.ids {
            Some(ids) if !ids.is_empty() => {
                sqlx::query(
                    "UPDATE notifications SET read_at=now()
                     WHERE read_at IS NULL AND id = ANY($1)",
                )
                .bind(ids)
                .execute(pool)
                .await?
            }
            _ => return Ok(0),
        }
    };
    Ok(result.rows_affected() as i64)
}
