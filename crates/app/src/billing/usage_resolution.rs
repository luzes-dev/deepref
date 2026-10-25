use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};
use deepref_domain::UserId;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageResolutionAction {
    ManualReconciliation,
    ReopenedRetry,
    Refused,
}

impl UsageResolutionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualReconciliation => "manual_reconciliation",
            Self::ReopenedRetry => "reopened_retry",
            Self::Refused => "refused",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual_reconciliation" => Some(Self::ManualReconciliation),
            "reopened_retry" => Some(Self::ReopenedRetry),
            "refused" => Some(Self::Refused),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageResolutionReason {
    ExternalAdjustmentConfirmed,
    ProviderDuplicateVerified,
    ManualCreditIssued,
    RejectedUnrecoverable,
}

impl UsageResolutionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExternalAdjustmentConfirmed => "external_adjustment_confirmed",
            Self::ProviderDuplicateVerified => "provider_duplicate_verified",
            Self::ManualCreditIssued => "manual_credit_issued",
            Self::RejectedUnrecoverable => "rejected_unrecoverable",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "external_adjustment_confirmed" => Some(Self::ExternalAdjustmentConfirmed),
            "provider_duplicate_verified" => Some(Self::ProviderDuplicateVerified),
            "manual_credit_issued" => Some(Self::ManualCreditIssued),
            "rejected_unrecoverable" => Some(Self::RejectedUnrecoverable),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UsageExportHoldStatus {
    pub fact_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub settled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub delivery_state: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolution_id: Option<Uuid>,
    pub resolution_action: Option<String>,
    pub resolution_reason: Option<String>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Backward compatibility alias for `UsageExportHoldStatus`.
pub type UnresolvedUsageExport = UsageExportHoldStatus;

#[derive(Debug, Error)]
pub enum UsageResolutionError {
    #[error("outbox event not found")]
    NotFound,
    #[error("outbox event is not in quarantined state")]
    NotQuarantined,
    #[error("fact or resolution UUID is already recorded")]
    AlreadyResolved,
    #[error("invalid resolution argument: {0}")]
    InvalidArgument(&'static str),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub async fn list_usage_export_holds(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<UsageExportHoldStatus>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT e.event_id AS fact_id,
                  (e.payload->>'workspace_id')::uuid AS workspace_id,
                  (e.payload->>'settled_at')::timestamptz AS settled_at,
                  e.delivery_state,
                  e.attempts,
                  e.last_error,
                  e.created_at,
                  r.resolution_id,
                  r.action AS resolution_action,
                  r.reason AS resolution_reason,
                  r.resolved_at
           FROM outbox_events e
           LEFT JOIN billing_usage_resolutions r ON r.fact_id = e.event_id
           WHERE e.message_type = 'ai_usage.fact'
             AND (e.delivery_state = 'quarantined' OR r.fact_id IS NOT NULL)
           ORDER BY e.created_at DESC
           LIMIT $1"#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let items = rows
        .into_iter()
        .map(|row| UsageExportHoldStatus {
            fact_id: row.get("fact_id"),
            workspace_id: row.get("workspace_id"),
            settled_at: row.get("settled_at"),
            delivery_state: row.get("delivery_state"),
            attempts: row.get("attempts"),
            last_error: row.get("last_error"),
            created_at: row.get("created_at"),
            resolution_id: row.get("resolution_id"),
            resolution_action: row.get("resolution_action"),
            resolution_reason: row.get("resolution_reason"),
            resolved_at: row.get("resolved_at"),
        })
        .collect();

    Ok(items)
}

pub async fn list_unresolved_usage_exports(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<UsageExportHoldStatus>, sqlx::Error> {
    list_usage_export_holds(pool, limit).await
}

pub async fn resolve_usage_export_hold(
    pool: &PgPool,
    fact_id: Uuid,
    operator_user_id: UserId,
    resolution_id: Uuid,
    action: UsageResolutionAction,
    reason: UsageResolutionReason,
    notes: Option<String>,
) -> Result<(), UsageResolutionError> {
    if notes.as_ref().is_some_and(|n| n.len() > 500) {
        return Err(UsageResolutionError::InvalidArgument(
            "notes must not exceed 500 characters",
        ));
    }

    let mut tx = pool.begin().await?;

    // 1. Lock and inspect outbox_events row
    let event_row = sqlx::query(
        "SELECT delivery_state, (payload->>'workspace_id')::uuid AS workspace_id FROM outbox_events WHERE event_id = $1 AND message_type = 'ai_usage.fact' FOR UPDATE",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (delivery_state, workspace_id): (String, Option<Uuid>) = match event_row {
        Some(row) => (row.get("delivery_state"), row.get("workspace_id")),
        None => return Err(UsageResolutionError::NotFound),
    };

    if delivery_state != "quarantined" {
        return Err(UsageResolutionError::NotQuarantined);
    }

    let workspace_id = match workspace_id {
        Some(w) => w,
        None => {
            return Err(UsageResolutionError::InvalidArgument(
                "missing workspace_id in payload",
            ));
        }
    };

    // 2. Check if already resolved
    let existing_res = sqlx::query(
        "SELECT 1 FROM billing_usage_resolutions WHERE fact_id = $1 OR resolution_id = $2",
    )
    .bind(fact_id)
    .bind(resolution_id)
    .fetch_optional(&mut *tx)
    .await?;

    if existing_res.is_some() {
        return Err(UsageResolutionError::AlreadyResolved);
    }

    // 3. Verify operator user exists
    let user_exists = sqlx::query("SELECT 1 FROM users WHERE id = $1")
        .bind(operator_user_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;

    if user_exists.is_none() {
        return Err(UsageResolutionError::InvalidArgument(
            "operator user not found",
        ));
    }

    // 4. Record into billing_usage_resolutions
    sqlx::query(
        "INSERT INTO billing_usage_resolutions (fact_id, workspace_id, resolution_id, resolved_by, action, reason, notes) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(fact_id)
    .bind(workspace_id)
    .bind(resolution_id)
    .bind(operator_user_id.as_uuid())
    .bind(action.as_str())
    .bind(reason.as_str())
    .bind(notes)
    .execute(&mut *tx)
    .await?;

    // 5. Update outbox according to action
    match action {
        UsageResolutionAction::ReopenedRetry => {
            // Monotonic fence invariant: advance claim_generation so any stale worker
            // pre-resolution attempt is rejected when completing or failing.
            sqlx::query(
                "UPDATE outbox_events
                 SET delivery_state = 'pending', attempts = 0, next_attempt_at = now(),
                     lease_owner = NULL, lease_expires_at = NULL, last_error = NULL,
                     claim_generation = claim_generation + 1
                 WHERE event_id = $1",
            )
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;

            // Targeted consumer reset: ONLY reset authorized consumer receipt ('ai_usage_export').
            // Do NOT touch other consumers or reset monotonic fences (attempt_generation / settled_generation).
            sqlx::query(
                "UPDATE outbox_receipts
                 SET state = 'retrying', attempts = 0, next_attempt_at = now(), last_error = NULL
                 WHERE event_id = $1 AND consumer = 'ai_usage_export' AND state = 'quarantined'",
            )
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;

            // Reset dispatch intent in billing_usage_fact_targets so this resolution authorizes
            // a single fresh dispatch attempt.
            sqlx::query(
                "UPDATE billing_usage_fact_targets
                 SET dispatch_claimed_at = NULL, dispatch_state = 'bound'
                 WHERE fact_id = $1",
            )
            .bind(fact_id)
            .execute(&mut *tx)
            .await?;
        }
        UsageResolutionAction::ManualReconciliation => {
            // Distinguish manually resolved from provider delivered
            sqlx::query("UPDATE outbox_events SET last_error = $2 WHERE event_id = $1")
                .bind(fact_id)
                .bind(format!("manually resolved: resolution_id={resolution_id}"))
                .execute(&mut *tx)
                .await?;
        }
        UsageResolutionAction::Refused => {
            sqlx::query("UPDATE outbox_events SET last_error = $2 WHERE event_id = $1")
                .bind(fact_id)
                .bind(format!(
                    "refused unrecoverable: resolution_id={resolution_id}"
                ))
                .execute(&mut *tx)
                .await?;
        }
    }

    // 6. Record audit event: closed 0168 schema 'billing.changed' with 'review_resolved'
    insert_admin_audit(
        &mut tx,
        "billing.changed",
        AuditPrincipal::User(operator_user_id),
        None,
        Some(workspace_id),
        None,
        serde_json::json!({
            "operation_id": resolution_id.to_string(),
            "reason": "review_resolved",
        }),
        AuditRequest::default(),
    )
    .await
    .map_err(|_| UsageResolutionError::InvalidArgument("failed to write audit log"))?;

    tx.commit().await?;
    Ok(())
}
