use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::AssistantError;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistantPlanRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub status: String,
    pub summary: String,
    pub actions: Value,
    pub results: Option<Value>,
    pub error: Option<String>,
    pub created_by_kind: String,
    pub created_by_id: String,
    pub model: String,
    pub prompt_version: String,
    pub evidence: Value,
    pub created_at: DateTime<Utc>,
    pub resolved_by_kind: Option<String>,
    pub resolved_by_id: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewAssistantPlan {
    pub id: Uuid,
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    pub summary: String,
    pub actions: Value,
    pub created_by_kind: String,
    pub created_by_id: String,
    pub model: String,
    pub prompt_version: String,
    pub evidence: Value,
}

macro_rules! columns {
    () => {
        "id, project_id, conversation_id, status, summary, actions, results, error,
         created_by_kind, created_by_id, model, prompt_version, evidence, created_at,
         resolved_by_kind, resolved_by_id, resolved_at"
    };
}

fn plan_from_row(row: &PgRow) -> AssistantPlanRecord {
    AssistantPlanRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        conversation_id: row.get("conversation_id"),
        status: row.get("status"),
        summary: row.get("summary"),
        actions: row.get("actions"),
        results: row.get("results"),
        error: row.get("error"),
        created_by_kind: row.get("created_by_kind"),
        created_by_id: row.get("created_by_id"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        evidence: row.get("evidence"),
        created_at: row.get("created_at"),
        resolved_by_kind: row.get("resolved_by_kind"),
        resolved_by_id: row.get("resolved_by_id"),
        resolved_at: row.get("resolved_at"),
    }
}

pub async fn create_assistant_plan(
    pool: &PgPool,
    plan: &NewAssistantPlan,
) -> Result<AssistantPlanRecord, AssistantError> {
    let mut tx = pool.begin().await?;
    let record = create_assistant_plan_tx(&mut tx, plan).await?;
    tx.commit().await?;
    Ok(record)
}

pub(crate) async fn create_assistant_plan_tx(
    tx: &mut Transaction<'_, Postgres>,
    plan: &NewAssistantPlan,
) -> Result<AssistantPlanRecord, AssistantError> {
    let row = sqlx::query(concat!(
        "INSERT INTO assistant_plans
           (id, project_id, conversation_id, summary, actions, created_by_kind, created_by_id,
            model, prompt_version, evidence)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
         RETURNING ",
        columns!()
    ))
    .bind(plan.id)
    .bind(plan.project_id)
    .bind(plan.conversation_id)
    .bind(&plan.summary)
    .bind(&plan.actions)
    .bind(&plan.created_by_kind)
    .bind(&plan.created_by_id)
    .bind(&plan.model)
    .bind(&plan.prompt_version)
    .bind(&plan.evidence)
    .fetch_one(&mut **tx)
    .await?;
    Ok(plan_from_row(&row))
}

pub async fn get_assistant_plan(
    pool: &PgPool,
    project_id: Uuid,
    plan_id: Uuid,
) -> Result<Option<AssistantPlanRecord>, AssistantError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM assistant_plans WHERE id = $1 AND project_id = $2"
    ))
    .bind(plan_id)
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(plan_from_row))
}

/// Atomically moves a pending plan to `confirmed` (execution claimed) or
/// `rejected`. Returns `None` when the plan is not pending any more, which is
/// what makes confirm exactly-once.
pub async fn claim_assistant_plan(
    pool: &PgPool,
    project_id: Uuid,
    plan_id: Uuid,
    next_status: &str,
    actor_kind: &str,
    actor_id: &str,
) -> Result<Option<AssistantPlanRecord>, AssistantError> {
    debug_assert!(matches!(next_status, "confirmed" | "rejected"));
    let row = sqlx::query(concat!(
        "UPDATE assistant_plans
         SET status = $3, resolved_by_kind = $4, resolved_by_id = $5, resolved_at = now()
         WHERE id = $1 AND project_id = $2 AND status = 'pending'
         RETURNING ",
        columns!()
    ))
    .bind(plan_id)
    .bind(project_id)
    .bind(next_status)
    .bind(actor_kind)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(plan_from_row))
}

/// Stores refreshed per-step results and the plan status after the review runs
/// behind queued steps have moved on. Unlike `finish_assistant_plan` it may be
/// called again on a plan that is already executed or failed.
pub async fn update_assistant_plan_results(
    pool: &PgPool,
    plan_id: Uuid,
    status: &str,
    results: &Value,
    error: Option<&str>,
) -> Result<AssistantPlanRecord, AssistantError> {
    debug_assert!(matches!(status, "executed" | "failed"));
    let row = sqlx::query(concat!(
        "UPDATE assistant_plans SET status = $2, results = $3, error = $4
         WHERE id = $1 RETURNING ",
        columns!()
    ))
    .bind(plan_id)
    .bind(status)
    .bind(results)
    .bind(error)
    .fetch_one(pool)
    .await?;
    Ok(plan_from_row(&row))
}

pub async fn finish_assistant_plan(
    pool: &PgPool,
    plan_id: Uuid,
    status: &str,
    results: &Value,
    error: Option<&str>,
) -> Result<AssistantPlanRecord, AssistantError> {
    debug_assert!(matches!(status, "executed" | "failed"));
    let row = sqlx::query(concat!(
        "UPDATE assistant_plans SET status = $2, results = $3, error = $4
         WHERE id = $1 RETURNING ",
        columns!()
    ))
    .bind(plan_id)
    .bind(status)
    .bind(results)
    .bind(error)
    .fetch_one(pool)
    .await?;
    Ok(plan_from_row(&row))
}
