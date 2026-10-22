use std::time::Duration;

use deepref_ai::{
    AiError, AiFuture, BudgetSnapshot, ModelProfile, UsageLedger, UsageObservation,
    UsagePriceSnapshot, UsageReservation, UsageReservationRequest, UsageTopUpRequest,
    ZeroLiabilityProof,
};
use deepref_domain::{JobKind, ProjectId, UserId, WorkspaceId};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::jobs::park_queued_jobs;

/// Default monthly AI budget (US$5.00) in micro-dollars; mirrors the column
/// default in migration 0027.
pub const DEFAULT_AI_MONTHLY_BUDGET_MICROS: i64 = 5_000_000;
/// A durable run cannot spend more than US$100 even when its workspace has
/// room. Operators may lower this per run through trusted host attribution.
pub const DEFAULT_AI_RUN_LIMIT_MICROS: i64 = 100_000_000;
const PLATFORM_USAGE_ID: Uuid = Uuid::from_u128(1);

#[derive(Debug, Error)]
pub enum AiUsageError {
    #[error("project not found")]
    ProjectNotFound,
    #[error("workspace not found")]
    WorkspaceNotFound,
    #[error("budget must be between 0 and 100000 USD")]
    InvalidBudget,
    #[error("operator identity is invalid")]
    InvalidOperator,
    #[error("workspace has no active AI overage block")]
    NoOverageBlock,
    #[error("AI usage reconciliation failed")]
    Reconciliation,
    #[error("AI usage historical backfill batch size is invalid")]
    InvalidBackfillBatchSize,
    #[error("AI hard caps cannot be enabled: {0}")]
    HardCapsGate(String),
    #[error("AI usage export cannot be enabled: {0}")]
    UsageExportGate(String),
    /// Contention, not failure: another rollout transition or an in-flight job
    /// write holds the rollout locks. Every lock the attempt took has been
    /// released; retrying the whole command is safe.
    #[error("an AI usage rollout transition is already in progress; retry")]
    RolloutBusy,
    #[error("AI usage reservation was not found or is already terminal")]
    ReservationNotResolvable,
    #[error("AI usage resolution is invalid")]
    InvalidResolution,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct PostgresUsageLedger {
    pool: PgPool,
}

impl PostgresUsageLedger {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }
}

pub async fn get_ai_budget(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<BudgetSnapshot, AiUsageError> {
    let row = sqlx::query(
        "SELECT p.ai_monthly_budget_micros AS budget,
                COALESCE((SELECT SUM(cost_micros)::bigint FROM ai_usage_ledger l
                          WHERE l.project_id = p.id
                            AND l.created_at >= date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'), 0) AS spent
         FROM projects p WHERE p.id = $1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AiUsageError::ProjectNotFound)?;
    Ok(BudgetSnapshot {
        budget_micros: row.get("budget"),
        spent_micros: row.get("spent"),
    })
}

pub async fn set_ai_budget(
    pool: &PgPool,
    project_id: Uuid,
    budget_micros: i64,
) -> Result<BudgetSnapshot, AiUsageError> {
    if !(0..=100_000_000_000).contains(&budget_micros) {
        return Err(AiUsageError::InvalidBudget);
    }
    let updated = sqlx::query("UPDATE projects SET ai_monthly_budget_micros = $2 WHERE id = $1")
        .bind(project_id)
        .bind(budget_micros)
        .execute(pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(AiUsageError::ProjectNotFound);
    }
    get_ai_budget(pool, project_id).await
}

fn db_failure() -> AiError {
    AiError::Persistence("AI usage ledger operation failed".to_owned())
}

fn to_i64(value: u64) -> Result<i64, AiError> {
    i64::try_from(value).map_err(|_| AiError::InvalidUsageBound)
}

fn optional_to_i64(value: Option<u64>) -> Result<Option<i64>, AiError> {
    value.map(to_i64).transpose()
}

const RUN_BUCKET_PERIOD: chrono::NaiveDate = chrono::NaiveDate::from_ymd_opt(1, 1, 1)
    .expect("the lifetime run bucket uses the first representable date");

#[derive(Debug, Clone, Copy)]
struct LockedUsageBucket {
    limit_micros: i64,
    spent_micros: i64,
    reserved_micros: i64,
}

struct StoredUsageEvidence {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    commercial_units: Option<i64>,
    provider_request_id: Option<String>,
    provider_response_id: Option<String>,
}

async fn lock_project_bucket_lifecycle(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Option<Uuid>,
) -> Result<(), AiError> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(project_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(|_| db_failure())?;
    Ok(())
}

async fn ensure_bucket_from_limit(
    tx: &mut Transaction<'_, Postgres>,
    scope_kind: &str,
    scope_id: Uuid,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    period_start: chrono::NaiveDate,
) -> Result<(), AiError> {
    sqlx::query(
        "INSERT INTO ai_usage_period_buckets
           (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
         SELECT $1, $2, $3, $4, $5, limit_micros
         FROM ai_usage_limits WHERE scope_kind = $1 AND scope_id = $2
         ON CONFLICT (scope_kind, scope_id, period_start) DO NOTHING",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(period_start)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    Ok(())
}

async fn ensure_run_bucket(
    tx: &mut Transaction<'_, Postgres>,
    run_budget_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    limit_micros: i64,
) -> Result<(), AiError> {
    sqlx::query(
        "INSERT INTO ai_usage_period_buckets
           (scope_kind, scope_id, workspace_id, project_id, run_budget_id,
            period_start, limit_micros)
         VALUES ('run', $1, $2, $3, $1, $4, $5)
         ON CONFLICT (scope_kind, scope_id, period_start) DO NOTHING",
    )
    .bind(run_budget_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(RUN_BUCKET_PERIOD)
    .bind(limit_micros)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    Ok(())
}

async fn lock_bucket(
    tx: &mut Transaction<'_, Postgres>,
    scope_kind: &str,
    scope_id: Uuid,
    period_start: chrono::NaiveDate,
) -> Result<LockedUsageBucket, AiError> {
    sqlx::query(
        "SELECT limit_micros, spent_micros, reserved_micros
         FROM ai_usage_period_buckets
         WHERE scope_kind = $1 AND scope_id = $2 AND period_start = $3
         FOR UPDATE",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(period_start)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| db_failure())?
    .map(|row| LockedUsageBucket {
        limit_micros: row.get("limit_micros"),
        spent_micros: row.get("spent_micros"),
        reserved_micros: row.get("reserved_micros"),
    })
    .ok_or_else(db_failure)
}

async fn lock_workspace_admission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, AiError> {
    sqlx::query(
        "INSERT INTO ai_usage_workspace_admission_state (workspace_id)
         VALUES ($1) ON CONFLICT (workspace_id) DO NOTHING",
    )
    .bind(workspace_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    sqlx::query_scalar(
        "SELECT blocked FROM ai_usage_workspace_admission_state
         WHERE workspace_id = $1 FOR UPDATE",
    )
    .bind(workspace_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| db_failure())?
    .ok_or_else(db_failure)
}

async fn reserve_bucket_amount(
    tx: &mut Transaction<'_, Postgres>,
    scope_kind: &str,
    scope_id: Uuid,
    period_start: chrono::NaiveDate,
    amount_micros: i64,
) -> Result<(), AiError> {
    let available: Option<Uuid> = sqlx::query_scalar(
        "UPDATE ai_usage_period_buckets
         SET reserved_micros = reserved_micros + $4, updated_at = now()
         WHERE scope_kind = $1 AND scope_id = $2 AND period_start = $3
           AND limit_micros >= $4
           AND spent_micros <= limit_micros - $4
           AND reserved_micros <= limit_micros - spent_micros - $4
         RETURNING scope_id",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(period_start)
    .bind(amount_micros)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    available.map(|_| ()).ok_or(AiError::UsageLimitExceeded)
}

fn ai_usage_price(row: &sqlx::postgres::PgRow) -> UsagePriceSnapshot {
    UsagePriceSnapshot {
        version: row.get("price_version"),
        input_micros_per_million_tokens: row.get("input_micros_per_million_tokens"),
        output_micros_per_million_tokens: row.get("output_micros_per_million_tokens"),
        reasoning_micros_per_million_tokens: row.get("reasoning_micros_per_million_tokens"),
    }
}

async fn adjust_bucket(
    tx: &mut Transaction<'_, Postgres>,
    scope_kind: &str,
    scope_id: Uuid,
    period_start: chrono::NaiveDate,
    spent_delta: i64,
    reserved_delta: i64,
) -> Result<(), AiError> {
    let changed = sqlx::query(
        "UPDATE ai_usage_period_buckets
         SET spent_micros = spent_micros + $4,
             reserved_micros = reserved_micros + $5,
             updated_at = now()
         WHERE scope_kind = $1 AND scope_id = $2 AND period_start = $3
           AND spent_micros + $4 >= 0 AND reserved_micros + $5 >= 0",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(period_start)
    .bind(spent_delta)
    .bind(reserved_delta)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    if changed.rows_affected() != 1 {
        return Err(db_failure());
    }
    Ok(())
}

async fn hierarchy_overage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    run_budget_id: Uuid,
    period_start: chrono::NaiveDate,
) -> Result<i64, AiError> {
    sqlx::query_scalar(
        "SELECT COALESCE(MAX(GREATEST(0, spent_micros + reserved_micros - limit_micros)), 0)
         FROM ai_usage_period_buckets
         WHERE ((period_start = $1 AND
            ((scope_kind = 'platform' AND scope_id = $2)
             OR (scope_kind = 'workspace' AND scope_id = $3)
             OR (scope_kind = 'project' AND scope_id = $4)))
            OR (scope_kind = 'run' AND scope_id = $5 AND period_start = $6))",
    )
    .bind(period_start)
    .bind(PLATFORM_USAGE_ID)
    .bind(workspace_id)
    .bind(project_id)
    .bind(run_budget_id)
    .bind(RUN_BUCKET_PERIOD)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| db_failure())
}

async fn block_workspace(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    reservation_id: Uuid,
    overage_micros: i64,
) -> Result<(), AiError> {
    if overage_micros <= 0 {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO ai_usage_workspace_blocks (workspace_id, overage_micros, reservation_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (workspace_id) DO UPDATE
           SET overage_micros = ai_usage_workspace_blocks.overage_micros + EXCLUDED.overage_micros,
               reservation_id = EXCLUDED.reservation_id,
               blocked_at = LEAST(ai_usage_workspace_blocks.blocked_at, now()),
               cleared_at = NULL, cleared_by_user_id = NULL, clear_reason = NULL",
    )
    .bind(workspace_id)
    .bind(overage_micros)
    .bind(reservation_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    sqlx::query(
        "INSERT INTO ai_usage_workspace_admission_state (workspace_id, blocked)
         VALUES ($1, true)
         ON CONFLICT (workspace_id) DO UPDATE SET blocked = true, updated_at = now()",
    )
    .bind(workspace_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    Ok(())
}

async fn garbage_collect_detached_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Option<Uuid>,
) -> Result<(), AiError> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    let detached: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ai_usage_detached_projects WHERE project_id = $1)",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    if !detached {
        return Ok(());
    }
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ai_usage_reservations
         WHERE project_id = $1 AND state IN ('open', 'observed', 'unresolved'))",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    if pending {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM ai_usage_period_buckets
         WHERE project_id = $1 AND scope_kind IN ('project', 'run')",
    )
    .bind(project_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| db_failure())?;
    sqlx::query("DELETE FROM ai_usage_limits WHERE scope_kind = 'project' AND scope_id = $1")
        .bind(project_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| db_failure())?;
    sqlx::query("DELETE FROM ai_usage_detached_projects WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| db_failure())?;
    Ok(())
}

impl UsageLedger for PostgresUsageLedger {
    fn reserve<'a>(&'a self, request: UsageReservationRequest) -> AiFuture<'a, UsageReservation> {
        Box::pin(async move {
            let reserved_cost = request.reserved_cost_micros()?;
            if !matches!(
                request.purpose.as_str(),
                "structured" | "chat" | "assistant"
            ) {
                return Err(AiError::InvalidUsageBound);
            }
            let input_bound = to_i64(request.input_token_bound.ok_or(AiError::UnboundedInput)?)?;
            let max_output = to_i64(request.max_output_tokens.ok_or(AiError::UnboundedOutput)?)?;
            let max_reasoning = request
                .max_reasoning_tokens
                .map(to_i64)
                .transpose()?
                .unwrap_or(0);
            let mut tx = self.pool.begin().await.map_err(|_| db_failure())?;
            let project_id = request.project_id.map(ProjectId::as_uuid);
            let requested_workspace = request.attribution.workspace_id.map(WorkspaceId::as_uuid);
            let project_workspace = if let Some(project_id) = project_id {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT workspace_id FROM projects WHERE id = $1 FOR KEY SHARE",
                )
                .bind(project_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| db_failure())?
            } else {
                None
            };
            if project_id.is_some() && project_workspace.is_none() {
                return Err(AiError::MissingUsageWorkspace);
            }
            if requested_workspace.is_some()
                && project_workspace.is_some()
                && requested_workspace != project_workspace
            {
                return Err(AiError::InvalidContext(
                    "usage tenant does not own project".to_owned(),
                ));
            }
            let workspace_id = requested_workspace
                .or(project_workspace)
                .ok_or(AiError::MissingUsageWorkspace)?;
            let workspace_exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workspaces WHERE id = $1)")
                    .bind(workspace_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|_| db_failure())?;
            if !workspace_exists {
                return Err(AiError::MissingUsageWorkspace);
            }
            let run_limit = request
                .attribution
                .run_limit_micros
                .unwrap_or(DEFAULT_AI_RUN_LIMIT_MICROS);
            if run_limit < 0 {
                return Err(AiError::InvalidUsageBound);
            }
            let reservation_id = request.reservation_id;
            let period_start: chrono::NaiveDate =
                sqlx::query_scalar("SELECT date_trunc('month', now() AT TIME ZONE 'UTC')::date")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|_| db_failure())?;
            ensure_bucket_from_limit(
                &mut tx,
                "platform",
                PLATFORM_USAGE_ID,
                None,
                None,
                period_start,
            )
            .await?;
            ensure_bucket_from_limit(
                &mut tx,
                "workspace",
                workspace_id,
                Some(workspace_id),
                None,
                period_start,
            )
            .await?;
            if let Some(project_id) = project_id {
                ensure_bucket_from_limit(
                    &mut tx,
                    "project",
                    project_id,
                    Some(workspace_id),
                    Some(project_id),
                    period_start,
                )
                .await?;
            }
            ensure_run_bucket(
                &mut tx,
                request.attribution.run_budget_id,
                workspace_id,
                project_id,
                run_limit,
            )
            .await?;

            // Lock every admission bucket in one order. Limits come from the
            // locked rows, so a cap reduction cannot be overwritten by a
            // pre-lock read from ai_usage_limits.
            let platform =
                lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start).await?;
            let workspace = lock_bucket(&mut tx, "workspace", workspace_id, period_start).await?;
            let workspace_blocked = lock_workspace_admission(&mut tx, workspace_id).await?;
            let project = if let Some(project_id) = project_id {
                Some(lock_bucket(&mut tx, "project", project_id, period_start).await?)
            } else {
                None
            };
            let mut run = lock_bucket(
                &mut tx,
                "run",
                request.attribution.run_budget_id,
                RUN_BUCKET_PERIOD,
            )
            .await?;
            sqlx::query(
                "UPDATE ai_usage_period_buckets
                 SET limit_micros = LEAST(limit_micros, $3), updated_at = now()
                 WHERE scope_kind = 'run' AND scope_id = $1 AND period_start = $2",
            )
            .bind(request.attribution.run_budget_id)
            .bind(RUN_BUCKET_PERIOD)
            .bind(run_limit)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            run.limit_micros = run.limit_micros.min(run_limit);

            let caps_enforced: bool = sqlx::query_scalar(
                "SELECT ai_hard_caps_enforced FROM ai_usage_rollout_state
                 WHERE singleton",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?
            .ok_or_else(db_failure)?;
            if !caps_enforced {
                tracing::warn!(
                    reason = "hard_caps_rollout_gate",
                    reservation_id = %reservation_id,
                    "AI usage reservation denied before provider dispatch"
                );
                return Err(AiError::HardCapsDisabled);
            }

            // Block state is a locked workspace-scoped admission row taken
            // directly after the workspace bucket. Settlement uses the same
            // sequence, including when it accounts against a prior month.
            if workspace_blocked {
                return Err(AiError::WorkspaceAiBlocked);
            }
            let mut buckets = vec![
                ("platform", PLATFORM_USAGE_ID, platform),
                ("workspace", workspace_id, workspace),
            ];
            if let (Some(project_id), Some(project)) = (project_id, project) {
                buckets.push(("project", project_id, project));
            }
            buckets.push(("run", request.attribution.run_budget_id, run));
            for (scope_kind, scope_id, bucket) in buckets {
                if bucket.limit_micros < reserved_cost
                    || bucket.spent_micros > bucket.limit_micros - reserved_cost
                    || bucket.reserved_micros
                        > bucket.limit_micros - bucket.spent_micros - reserved_cost
                {
                    return Err(AiError::UsageLimitExceeded);
                }
                reserve_bucket_amount(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    if scope_kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    reserved_cost,
                )
                .await?;
            }

            let (principal_kind, principal_id) = request
                .attribution
                .principal
                .as_ref()
                .map(|principal| (Some(principal.kind()), Some(principal.id())))
                .unwrap_or((None, None));
            let inserted = sqlx::query(
                "INSERT INTO ai_usage_reservations
                 (id, workspace_id, project_id, principal_kind, principal_id, run_id, run_budget_id, step_id,
                  job_id, attempt_fence, profile, provider, model, purpose, input_token_bound,
                  max_output_tokens, max_reasoning_tokens, input_micros_per_million_tokens,
                  output_micros_per_million_tokens, reasoning_micros_per_million_tokens,
                  price_version, request_fingerprint, reserved_cost_micros, period_start)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24)",
            )
            .bind(reservation_id)
            .bind(workspace_id)
            .bind(project_id)
            .bind(principal_kind)
            .bind(principal_id)
            .bind(request.attribution.run_id)
            .bind(request.attribution.run_budget_id)
            .bind(&request.attribution.step_id)
            .bind(request.attribution.job_id)
            .bind(request.attribution.attempt_fence)
            .bind(&request.profile)
            .bind(&request.provider)
            .bind(&request.model)
            .bind(&request.purpose)
            .bind(input_bound)
            .bind(max_output)
            .bind(max_reasoning)
            .bind(request.price.input_micros_per_million_tokens)
            .bind(request.price.output_micros_per_million_tokens)
            .bind(request.price.reasoning_micros_per_million_tokens)
            .bind(&request.price.version)
            .bind(&request.request_fingerprint)
            .bind(reserved_cost)
            .bind(period_start)
            .execute(&mut *tx)
            .await;
            if inserted.is_err() {
                return Err(db_failure());
            }
            tx.commit().await.map_err(|_| db_failure())?;
            tracing::info!(
                reserved_cost_micros = reserved_cost,
                "AI usage reservation accepted"
            );
            Ok(UsageReservation {
                id: reservation_id,
                workspace_id: WorkspaceId::new(workspace_id),
                period_start,
                reserved_cost_micros: reserved_cost,
                price: request.price,
            })
        })
    }

    fn top_up<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        request: UsageTopUpRequest,
    ) -> AiFuture<'a, UsageReservation> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(|_| db_failure())?;
            let row = sqlx::query(
                "SELECT workspace_id, project_id, run_id, run_budget_id, period_start, state,
                        reserved_cost_micros, input_token_bound, max_output_tokens,
                        max_reasoning_tokens, price_version,
                        input_micros_per_million_tokens, output_micros_per_million_tokens,
                        reasoning_micros_per_million_tokens
                 FROM ai_usage_reservations WHERE id = $1 FOR UPDATE",
            )
            .bind(reservation.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?
            .ok_or_else(db_failure)?;
            let state: String = row.get("state");
            if !matches!(state.as_str(), "open" | "observed") {
                return Err(AiError::InvalidUsageBound);
            }
            let price = ai_usage_price(&row);
            let additional_cost = request.reserved_cost_micros(&price)?;
            let new_reserved = row
                .get::<i64, _>("reserved_cost_micros")
                .checked_add(additional_cost)
                .ok_or(AiError::InvalidUsageBound)?;
            let input_bound = row
                .get::<i64, _>("input_token_bound")
                .checked_add(to_i64(request.additional_input_token_bound)?)
                .ok_or(AiError::InvalidUsageBound)?;
            let max_output = row
                .get::<i64, _>("max_output_tokens")
                .checked_add(to_i64(request.additional_output_tokens)?)
                .ok_or(AiError::InvalidUsageBound)?;
            let max_reasoning = row
                .get::<i64, _>("max_reasoning_tokens")
                .checked_add(to_i64(request.additional_reasoning_tokens)?)
                .ok_or(AiError::InvalidUsageBound)?;
            let workspace_id: Uuid = row.get("workspace_id");
            let project_id: Option<Uuid> = row.get("project_id");
            let run_budget_id: Uuid = row.get("run_budget_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            let detached: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM ai_usage_detached_projects WHERE project_id = $1)",
            )
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            if detached {
                return Err(AiError::InvalidUsageBound);
            }

            // Existing buckets are already initialized by the reservation.
            // Lock them explicitly in the same order used by reserve/set-cap.
            let platform =
                lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start).await?;
            let workspace = lock_bucket(&mut tx, "workspace", workspace_id, period_start).await?;
            let workspace_blocked = lock_workspace_admission(&mut tx, workspace_id).await?;
            let project = if let Some(project_id) = project_id {
                Some(lock_bucket(&mut tx, "project", project_id, period_start).await?)
            } else {
                None
            };
            let run = lock_bucket(&mut tx, "run", run_budget_id, RUN_BUCKET_PERIOD).await?;
            let caps_enforced: bool = sqlx::query_scalar(
                "SELECT ai_hard_caps_enforced FROM ai_usage_rollout_state
                 WHERE singleton",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?
            .ok_or_else(db_failure)?;
            if !caps_enforced {
                return Err(AiError::HardCapsDisabled);
            }
            if workspace_blocked {
                return Err(AiError::WorkspaceAiBlocked);
            }
            let mut buckets = vec![
                ("platform", PLATFORM_USAGE_ID, platform),
                ("workspace", workspace_id, workspace),
            ];
            if let (Some(project_id), Some(project)) = (project_id, project) {
                buckets.push(("project", project_id, project));
            }
            buckets.push(("run", run_budget_id, run));
            for (scope_kind, scope_id, bucket) in buckets {
                if bucket.limit_micros < additional_cost
                    || bucket.spent_micros > bucket.limit_micros - additional_cost
                    || bucket.reserved_micros
                        > bucket.limit_micros - bucket.spent_micros - additional_cost
                {
                    return Err(AiError::UsageLimitExceeded);
                }
                reserve_bucket_amount(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    if scope_kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    additional_cost,
                )
                .await?;
            }
            let updated = sqlx::query(
                "UPDATE ai_usage_reservations
                 SET reserved_cost_micros = $2, input_token_bound = $3,
                     max_output_tokens = $4, max_reasoning_tokens = $5,
                     request_fingerprint = $6
                 WHERE id = $1 AND state IN ('open', 'observed')",
            )
            .bind(reservation.id)
            .bind(new_reserved)
            .bind(input_bound)
            .bind(max_output)
            .bind(max_reasoning)
            .bind(&request.request_fingerprint)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            if updated.rows_affected() != 1 {
                return Err(db_failure());
            }
            tx.commit().await.map_err(|_| db_failure())?;
            tracing::info!(
                additional_cost_micros = additional_cost,
                "AI usage reservation topped up"
            );
            Ok(UsageReservation {
                id: reservation.id,
                workspace_id: WorkspaceId::new(workspace_id),
                period_start,
                reserved_cost_micros: new_reserved,
                price,
            })
        })
    }

    fn observe<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: UsageObservation,
    ) -> AiFuture<'a, ()> {
        Box::pin(async move {
            observation.validate()?;
            let has_token_usage = observation.has_token_usage();
            let changed = sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = CASE WHEN $2 OR
                       (observed_input_tokens IS NOT NULL AND observed_output_tokens IS NOT NULL)
                     THEN 'observed' ELSE 'unresolved' END,
                     unresolved_reason = CASE WHEN $2 OR
                       (observed_input_tokens IS NOT NULL AND observed_output_tokens IS NOT NULL)
                     THEN NULL ELSE 'provider_usage_missing' END,
                     observed_input_tokens = COALESCE($3, observed_input_tokens),
                     observed_output_tokens = COALESCE($4, observed_output_tokens),
                     observed_reasoning_tokens = COALESCE($5, observed_reasoning_tokens),
                     observed_commercial_units = COALESCE($6, observed_commercial_units),
                     provider_request_id = COALESCE($7, provider_request_id),
                     provider_response_id = COALESCE($8, provider_response_id)
                 WHERE id = $1 AND state IN ('open', 'observed', 'unresolved')",
            )
            .bind(reservation.id)
            .bind(has_token_usage)
            .bind(optional_to_i64(observation.input_tokens)?)
            .bind(optional_to_i64(observation.output_tokens)?)
            .bind(optional_to_i64(observation.reasoning_tokens)?)
            .bind(observation.commercial_units)
            .bind(&observation.provider_request_id)
            .bind(&observation.provider_response_id)
            .execute(&self.pool)
            .await
            .map_err(|_| db_failure())?;
            if changed.rows_affected() != 1 {
                return Err(db_failure());
            }
            tracing::info!("AI usage observation persisted");
            Ok(())
        })
    }

    fn settle<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: UsageObservation,
        current_price_version: &'a str,
    ) -> AiFuture<'a, ()> {
        Box::pin(async move {
            observation.validate()?;
            let mut tx = self.pool.begin().await.map_err(|_| db_failure())?;
            let row = sqlx::query(
                "SELECT workspace_id, project_id, run_id, run_budget_id, period_start, reserved_cost_micros,
                        recognized_cost_micros, overage_micros, state, price_version,
                        input_micros_per_million_tokens, output_micros_per_million_tokens,
                        reasoning_micros_per_million_tokens, provider, model,
                        observed_input_tokens, observed_output_tokens, observed_reasoning_tokens,
                        provider_request_id, provider_response_id
                 FROM ai_usage_reservations WHERE id = $1 FOR UPDATE",
            )
            .bind(reservation.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?
            .ok_or_else(db_failure)?;
            let state: String = row.get("state");
            if state == "settled" {
                tx.commit().await.map_err(|_| db_failure())?;
                return Ok(());
            }
            if state == "voided" {
                return Err(db_failure());
            }
            let price = ai_usage_price(&row);
            let actual = price.actual_cost_micros(&observation)?;
            let reserved_cost: i64 = row.get("reserved_cost_micros");
            let recognized_cost: i64 = row.get("recognized_cost_micros");
            let held_cost = reserved_cost.saturating_sub(recognized_cost).max(0);
            let workspace_id: Uuid = row.get("workspace_id");
            let project_id: Option<Uuid> = row.get("project_id");
            let run_budget_id: Uuid = row.get("run_budget_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            // Serialize final settlement with project detach before the
            // hierarchy buckets so detach and settlement always leave one
            // side responsible for detached-bucket garbage collection.
            lock_project_bucket_lifecycle(&mut tx, project_id).await?;
            let spent_delta = actual
                .checked_sub(recognized_cost)
                .ok_or(AiError::InvalidUsageBound)?;
            let held_delta = held_cost.checked_neg().ok_or(AiError::InvalidUsageBound)?;
            // Settlement takes the workspace bucket before observing or
            // setting a block. Admission and top-up use this same lock order.
            lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start).await?;
            lock_bucket(&mut tx, "workspace", workspace_id, period_start).await?;
            lock_workspace_admission(&mut tx, workspace_id).await?;
            if let Some(project_id) = project_id {
                lock_bucket(&mut tx, "project", project_id, period_start).await?;
            }
            lock_bucket(&mut tx, "run", run_budget_id, RUN_BUCKET_PERIOD).await?;
            let prior_hierarchy_overage = hierarchy_overage(
                &mut tx,
                workspace_id,
                project_id,
                run_budget_id,
                period_start,
            )
            .await?;
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_budget_id));
            for (scope_kind, scope_id) in scopes {
                adjust_bucket(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    if scope_kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    spent_delta,
                    held_delta,
                )
                .await?;
            }
            let overage = actual.saturating_sub(reserved_cost).max(0);
            let hierarchy_overage = hierarchy_overage(
                &mut tx,
                workspace_id,
                project_id,
                run_budget_id,
                period_start,
            )
            .await?;
            let prior_overage: i64 = row.get("overage_micros");
            let new_reservation_overage = overage.saturating_sub(prior_overage).max(0);
            let new_hierarchy_overage = hierarchy_overage
                .saturating_sub(prior_hierarchy_overage)
                .max(0);
            let block_amount = new_reservation_overage.max(new_hierarchy_overage);
            if block_amount > 0 {
                block_workspace(&mut tx, workspace_id, reservation.id, block_amount).await?;
            }
            let current_version = current_price_version.to_owned();
            let pricing_decision = if current_version == price.version {
                "reservation_snapshot_current"
            } else if current_version == "unavailable-at-recovery" {
                "reservation_snapshot_recovery"
            } else {
                "reservation_snapshot_changed"
            };
            sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = 'settled', observed_input_tokens = $2, observed_output_tokens = $3,
                     observed_reasoning_tokens = $4, observed_commercial_units = $5,
                     provider_request_id = COALESCE($6, provider_request_id),
                     provider_response_id = COALESCE($7, provider_response_id),
                     actual_cost_micros = $8, overage_micros = $9,
                     current_price_version = $10, pricing_decision = $11, settled_at = now()
                 WHERE id = $1",
            )
            .bind(reservation.id)
            .bind(optional_to_i64(observation.input_tokens)?)
            .bind(optional_to_i64(observation.output_tokens)?)
            .bind(optional_to_i64(observation.reasoning_tokens)?)
            .bind(observation.commercial_units)
            .bind(&observation.provider_request_id)
            .bind(&observation.provider_response_id)
            .bind(actual)
            .bind(overage)
            .bind(&current_version)
            .bind(pricing_decision)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;

            let fact_id = reservation.id;
            sqlx::query(
                "INSERT INTO ai_usage_ledger
                 (id, project_id, workspace_id, principal_kind, principal_id, run_id, run_budget_id, step_id,
                  profile, provider, model, purpose, input_tokens, output_tokens, reasoning_tokens,
                  cost_micros, commercial_units, price_version, current_price_version,
                  pricing_decision, period_start, reservation_id, provider_request_id, provider_response_id)
                 SELECT $1, project_id, workspace_id, principal_kind, principal_id, run_id, run_budget_id, step_id,
                        profile, provider, model, purpose, $2, $3, $4, $5, $6, price_version,
                        current_price_version, pricing_decision, period_start, id,
                        COALESCE($7, provider_request_id), COALESCE($8, provider_response_id)
                 FROM ai_usage_reservations WHERE id = $1",
            )
            .bind(fact_id)
            .bind(optional_to_i64(observation.input_tokens)?)
            .bind(optional_to_i64(observation.output_tokens)?)
            .bind(optional_to_i64(observation.reasoning_tokens)?)
            .bind(actual)
            .bind(observation.commercial_units)
            .bind(&observation.provider_request_id)
            .bind(&observation.provider_response_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            garbage_collect_detached_project(&mut tx, project_id).await?;
            tx.commit().await.map_err(|_| db_failure())?;
            if block_amount > 0 {
                tracing::warn!(
                    block_amount_micros = block_amount,
                    "workspace AI usage enforcement block recorded"
                );
            }
            tracing::info!(
                actual_cost_micros = actual,
                overage_micros = overage,
                "AI usage settlement committed"
            );
            Ok(())
        })
    }

    fn unresolved<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: Option<UsageObservation>,
        reason: &'a str,
    ) -> AiFuture<'a, ()> {
        Box::pin(async move {
            if reason.trim().is_empty() || reason.len() > 128 {
                return Err(AiError::InvalidUsageBound);
            }
            if let Some(observation) = observation.as_ref() {
                observation.validate()?;
            }
            let mut tx = self.pool.begin().await.map_err(|_| db_failure())?;
            let row = sqlx::query(
                "SELECT workspace_id, project_id, run_id, run_budget_id, period_start, reserved_cost_micros,
                        recognized_cost_micros, overage_micros, state, price_version,
                        input_micros_per_million_tokens, output_micros_per_million_tokens,
                        reasoning_micros_per_million_tokens, observed_input_tokens,
                        observed_output_tokens, observed_reasoning_tokens
                 FROM ai_usage_reservations WHERE id = $1 FOR UPDATE",
            )
            .bind(reservation.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?
            .ok_or_else(db_failure)?;
            let state: String = row.get("state");
            if matches!(state.as_str(), "settled" | "voided") {
                return Err(db_failure());
            }
            let prior_recognized: i64 = row.get("recognized_cost_micros");
            let reserved_cost: i64 = row.get("reserved_cost_micros");
            let workspace_id: Uuid = row.get("workspace_id");
            let project_id: Option<Uuid> = row.get("project_id");
            let run_budget_id: Uuid = row.get("run_budget_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            let price = ai_usage_price(&row);
            let complete_observation = observation.as_ref().is_some_and(|usage| {
                usage.has_token_usage()
                    && (price.reasoning_micros_per_million_tokens.is_none()
                        || usage.reasoning_tokens.is_some())
            });
            let known_cost = match observation.as_ref().filter(|_| complete_observation) {
                Some(observation) => price.actual_cost_micros(observation)?,
                None => prior_recognized,
            };
            let spent_delta = known_cost
                .checked_sub(prior_recognized)
                .ok_or(AiError::InvalidUsageBound)?;
            // Keep a hold only for the still-unrecognized part of the
            // reservation. If observed usage is already above the bound,
            // record the full usage and release only the hold that exists.
            let held_delta = prior_recognized
                .min(reserved_cost)
                .checked_sub(known_cost.min(reserved_cost))
                .ok_or(AiError::InvalidUsageBound)?;
            lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start).await?;
            lock_bucket(&mut tx, "workspace", workspace_id, period_start).await?;
            lock_workspace_admission(&mut tx, workspace_id).await?;
            if let Some(project_id) = project_id {
                lock_bucket(&mut tx, "project", project_id, period_start).await?;
            }
            lock_bucket(&mut tx, "run", run_budget_id, RUN_BUCKET_PERIOD).await?;
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_budget_id));
            for (scope_kind, scope_id) in scopes {
                adjust_bucket(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    if scope_kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    spent_delta,
                    held_delta,
                )
                .await?;
            }
            let known_overage = known_cost.saturating_sub(reserved_cost).max(0);
            let prior_overage: i64 = row.get("overage_micros");
            let new_overage = known_overage.saturating_sub(prior_overage).max(0);
            if new_overage > 0 {
                block_workspace(&mut tx, workspace_id, reservation.id, new_overage).await?;
            }
            let (input, output, reasoning, units, request_id, response_id) =
                if let Some(usage) = observation.as_ref() {
                    (
                        optional_to_i64(usage.input_tokens)?,
                        optional_to_i64(usage.output_tokens)?,
                        optional_to_i64(usage.reasoning_tokens)?,
                        usage.commercial_units,
                        usage.provider_request_id.as_deref(),
                        usage.provider_response_id.as_deref(),
                    )
                } else {
                    (None, None, None, None, None, None)
                };
            sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = 'unresolved', unresolved_reason = $2,
                     observed_input_tokens = COALESCE($3, observed_input_tokens),
                     observed_output_tokens = COALESCE($4, observed_output_tokens),
                     observed_reasoning_tokens = COALESCE($5, observed_reasoning_tokens),
                     observed_commercial_units = COALESCE($6, observed_commercial_units),
                     provider_request_id = COALESCE($7, provider_request_id),
                     provider_response_id = COALESCE($8, provider_response_id),
                     recognized_cost_micros = $9,
                     overage_micros = GREATEST(overage_micros, $10)
                 WHERE id = $1",
            )
            .bind(reservation.id)
            .bind(reason)
            .bind(input)
            .bind(output)
            .bind(reasoning)
            .bind(units)
            .bind(request_id)
            .bind(response_id)
            .bind(known_cost)
            .bind(known_overage)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            tx.commit().await.map_err(|_| db_failure())?;
            tracing::warn!(
                reason,
                reserved_cost_micros = reserved_cost,
                recognized_cost_micros = known_cost,
                overage_micros = known_overage,
                "AI usage liability remains unresolved"
            );
            Ok(())
        })
    }

    fn void_zero_liability<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        proof: ZeroLiabilityProof,
    ) -> AiFuture<'a, ()> {
        Box::pin(async move {
            proof.validate()?;
            let void_reason = proof.void_reason();
            let mut tx = self.pool.begin().await.map_err(|_| db_failure())?;
            let row = sqlx::query(
                "SELECT workspace_id, project_id, run_id, run_budget_id, period_start, reserved_cost_micros,
                        recognized_cost_micros, state, observed_input_tokens, observed_output_tokens,
                        observed_reasoning_tokens
                 FROM ai_usage_reservations WHERE id = $1 FOR UPDATE",
            )
            .bind(reservation.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?
            .ok_or_else(db_failure)?;
            let state: String = row.get("state");
            if matches!(state.as_str(), "settled" | "voided") {
                return Err(db_failure());
            }
            let recognized: i64 = row.get("recognized_cost_micros");
            let has_observed_tokens = [
                row.get::<Option<i64>, _>("observed_input_tokens"),
                row.get::<Option<i64>, _>("observed_output_tokens"),
                row.get::<Option<i64>, _>("observed_reasoning_tokens"),
            ]
            .into_iter()
            .flatten()
            .any(|tokens| tokens > 0);
            if has_observed_tokens || recognized > 0 {
                return Err(AiError::InvalidUsageBound);
            }
            let workspace_id: Uuid = row.get("workspace_id");
            let project_id: Option<Uuid> = row.get("project_id");
            let run_budget_id: Uuid = row.get("run_budget_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            lock_project_bucket_lifecycle(&mut tx, project_id).await?;
            let reserved_cost: i64 = row.get("reserved_cost_micros");
            let reserved_delta = reserved_cost
                .checked_neg()
                .ok_or(AiError::InvalidUsageBound)?;
            lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start).await?;
            lock_bucket(&mut tx, "workspace", workspace_id, period_start).await?;
            lock_workspace_admission(&mut tx, workspace_id).await?;
            if let Some(project_id) = project_id {
                lock_bucket(&mut tx, "project", project_id, period_start).await?;
            }
            lock_bucket(&mut tx, "run", run_budget_id, RUN_BUCKET_PERIOD).await?;
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_budget_id));
            for (scope_kind, scope_id) in scopes {
                adjust_bucket(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    if scope_kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    0,
                    reserved_delta,
                )
                .await?;
            }
            sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = 'voided', void_reason = $2, settled_at = now()
                 WHERE id = $1",
            )
            .bind(reservation.id)
            .bind(&void_reason)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            garbage_collect_detached_project(&mut tx, project_id).await?;
            tx.commit().await.map_err(|_| db_failure())?;
            tracing::info!(
                %void_reason,
                "zero-liability AI reservation voided"
            );
            Ok(())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiUsageLimitScope {
    Platform,
    Workspace(WorkspaceId),
    Project(ProjectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AiUsageBackfillSummary {
    pub processed_rows: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiUsageResolution {
    Settle { actual_cost_micros: i64 },
    Void { reason: String },
}

/// Processes one bounded, resumable batch of legacy usage facts. Each ledger
/// marker and its corresponding hierarchy increments commit together.
pub async fn backfill_ai_usage_legacy(
    pool: &PgPool,
    batch_size: usize,
) -> Result<AiUsageBackfillSummary, AiUsageError> {
    if !(1..=1000).contains(&batch_size) {
        return Err(AiUsageError::InvalidBackfillBatchSize);
    }
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        "SELECT l.id, l.project_id, COALESCE(l.workspace_id, p.workspace_id) AS workspace_id,
                COALESCE(l.period_start,
                  date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date) AS period_start,
                l.cost_micros
         FROM ai_usage_ledger l
         LEFT JOIN projects p ON p.id = l.project_id
         WHERE l.reservation_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM ai_usage_legacy_backfill b WHERE b.ledger_id = l.id)
         ORDER BY l.created_at, l.id
         FOR UPDATE OF l SKIP LOCKED
         LIMIT $1",
    )
    .bind(i64::try_from(batch_size).map_err(|_| AiUsageError::InvalidBackfillBatchSize)?)
    .fetch_all(&mut *tx)
    .await?;
    let mut processed = 0_u64;
    for row in rows {
        let ledger_id: Uuid = row.get("id");
        let project_id: Option<Uuid> = row.get("project_id");
        let workspace_id: Option<Uuid> = row.get("workspace_id");
        let period_start: chrono::NaiveDate = row.get("period_start");
        let cost_micros: i64 = row.get("cost_micros");
        if cost_micros < 0 {
            return Err(AiUsageError::Reconciliation);
        }
        sqlx::query(
            "UPDATE ai_usage_ledger SET workspace_id = COALESCE(workspace_id, $2),
                 period_start = COALESCE(period_start, $3),
                 run_budget_id = COALESCE(run_budget_id, run_id) WHERE id = $1",
        )
        .bind(ledger_id)
        .bind(workspace_id)
        .bind(period_start)
        .execute(&mut *tx)
        .await?;

        ensure_bucket_from_limit(
            &mut tx,
            "platform",
            PLATFORM_USAGE_ID,
            None,
            None,
            period_start,
        )
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
        lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start)
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;

        let workspace_has_limit = if let Some(workspace_id) = workspace_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM ai_usage_limits
                 WHERE scope_kind = 'workspace' AND scope_id = $1)",
            )
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await?
        } else {
            false
        };
        let workspace = if workspace_has_limit {
            let workspace_id = workspace_id.ok_or(AiUsageError::Reconciliation)?;
            ensure_bucket_from_limit(
                &mut tx,
                "workspace",
                workspace_id,
                Some(workspace_id),
                None,
                period_start,
            )
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
            let bucket = lock_bucket(&mut tx, "workspace", workspace_id, period_start)
                .await
                .map_err(|_| AiUsageError::Reconciliation)?;
            lock_workspace_admission(&mut tx, workspace_id)
                .await
                .map_err(|_| AiUsageError::Reconciliation)?;
            Some((workspace_id, bucket))
        } else {
            None
        };

        let project_has_limit = if let Some(project_id) = project_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM ai_usage_limits
                 WHERE scope_kind = 'project' AND scope_id = $1)",
            )
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?
        } else {
            false
        };
        let project = if project_has_limit {
            let project_id = project_id.ok_or(AiUsageError::Reconciliation)?;
            let workspace_id = workspace_id.ok_or(AiUsageError::Reconciliation)?;
            ensure_bucket_from_limit(
                &mut tx,
                "project",
                project_id,
                Some(workspace_id),
                Some(project_id),
                period_start,
            )
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
            Some(
                lock_bucket(&mut tx, "project", project_id, period_start)
                    .await
                    .map_err(|_| AiUsageError::Reconciliation)?,
            )
        } else {
            None
        };

        let prior_overage: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(GREATEST(0, spent_micros + reserved_micros - limit_micros)), 0)
             FROM ai_usage_period_buckets
             WHERE period_start = $1
               AND ((scope_kind = 'platform' AND scope_id = $2)
                 OR (scope_kind = 'workspace' AND scope_id = $3)
                 OR (scope_kind = 'project' AND scope_id = $4))",
        )
        .bind(period_start)
        .bind(PLATFORM_USAGE_ID)
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;

        adjust_bucket(
            &mut tx,
            "platform",
            PLATFORM_USAGE_ID,
            period_start,
            cost_micros,
            0,
        )
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
        if let Some((workspace_id, _)) = workspace {
            adjust_bucket(
                &mut tx,
                "workspace",
                workspace_id,
                period_start,
                cost_micros,
                0,
            )
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
        }
        if let (Some(project_id), Some(_)) = (project_id, project) {
            adjust_bucket(&mut tx, "project", project_id, period_start, cost_micros, 0)
                .await
                .map_err(|_| AiUsageError::Reconciliation)?;
        }
        sqlx::query(
            "INSERT INTO ai_usage_legacy_backfill (ledger_id) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(ledger_id)
        .execute(&mut *tx)
        .await?;

        if let Some((workspace_id, _)) = workspace {
            let current_overage: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(GREATEST(0, spent_micros + reserved_micros - limit_micros)), 0)
                 FROM ai_usage_period_buckets WHERE period_start = $1 AND
                   ((scope_kind = 'platform' AND scope_id = $2)
                    OR (scope_kind = 'workspace' AND scope_id = $3)
                    OR (scope_kind = 'project' AND scope_id = $4))",
            )
            .bind(period_start)
            .bind(PLATFORM_USAGE_ID)
            .bind(workspace_id)
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
            let overage = current_overage.saturating_sub(prior_overage).max(0);
            if overage > 0 {
                sqlx::query(
                    "UPDATE ai_usage_workspace_admission_state
                     SET blocked = true, updated_at = now() WHERE workspace_id = $1",
                )
                .bind(workspace_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO ai_usage_workspace_blocks (workspace_id, overage_micros, reservation_id)
                     VALUES ($1, $2, $3)
                     ON CONFLICT (workspace_id) DO UPDATE SET
                       overage_micros = ai_usage_workspace_blocks.overage_micros + EXCLUDED.overage_micros,
                       reservation_id = EXCLUDED.reservation_id,
                       blocked_at = LEAST(ai_usage_workspace_blocks.blocked_at, now()),
                       cleared_at = NULL, cleared_by_user_id = NULL, clear_reason = NULL",
                )
                .bind(workspace_id)
                .bind(overage)
                .bind(ledger_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        processed = processed.saturating_add(1);
    }
    let more_rows: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ai_usage_ledger l
         WHERE l.reservation_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM ai_usage_legacy_backfill b WHERE b.ledger_id = l.id))",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE ai_usage_backfill_state
         SET processed_rows = processed_rows + $1,
             completed_at = CASE WHEN $2 THEN NULL ELSE COALESCE(completed_at, now()) END,
             updated_at = now()
         WHERE singleton",
    )
    .bind(i64::try_from(processed).map_err(|_| AiUsageError::Reconciliation)?)
    .bind(more_rows)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(AiUsageBackfillSummary {
        processed_rows: processed,
        complete: !more_rows,
    })
}

/// The job-write guard installed by 0138: every `jobs` INSERT/UPDATE takes
/// this advisory lock shared before it touches a job row, and rollout
/// transitions take it exclusively while they change gates and park work.
const JOB_WRITE_ADVISORY_LOCK: (i32, i32) = (913, 9);
/// How many fresh attempts a rollout transition makes before giving up with
/// [`AiUsageError::RolloutBusy`]. Each attempt is a new transaction; every lock
/// it took is released before the next one starts.
const ROLLOUT_LOCK_ATTEMPTS: u32 = 4;
const ROLLOUT_LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Take the two rollout locks without ever waiting while holding either: the
/// job-write advisory lock `(913, 9)` exclusively (`pg_try_`, never blocking),
/// then the rollout singleton row `FOR UPDATE NOWAIT`.
///
/// A blocking exclusive acquisition deadlocks against ordinary transactions
/// that own a project row and then write a job (their shared statement lock),
/// against N-1 ledger writers that own the rollout row and then lock a project
/// row, and against the operator command in between: each holds one lock the
/// next one needs. Contention is therefore [`AiUsageError::RolloutBusy`],
/// returned only after this attempt has released every lock it took. The
/// caller receives the transaction with the rollout row locked (or `None`
/// when the singleton is missing) and decides what its own gate errors mean.
async fn begin_rollout_transition(
    pool: &PgPool,
) -> Result<(Transaction<'_, Postgres>, Option<PgRow>), AiUsageError> {
    let mut attempt = 1;
    loop {
        match try_begin_rollout_transition(pool).await {
            Err(AiUsageError::RolloutBusy) if attempt < ROLLOUT_LOCK_ATTEMPTS => {
                tokio::time::sleep(ROLLOUT_LOCK_RETRY_DELAY).await;
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

async fn try_begin_rollout_transition(
    pool: &PgPool,
) -> Result<(Transaction<'_, Postgres>, Option<PgRow>), AiUsageError> {
    let mut tx = pool.begin().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1, $2)")
        .bind(JOB_WRITE_ADVISORY_LOCK.0)
        .bind(JOB_WRITE_ADVISORY_LOCK.1)
        .fetch_one(&mut *tx)
        .await?;
    if !acquired {
        tx.rollback().await?;
        return Err(AiUsageError::RolloutBusy);
    }
    let rollout = sqlx::query(
        "SELECT singleton, ai_hard_caps_enforced, last_legacy_ledger_insert_at,
                ai_usage_export_activated
         FROM ai_usage_rollout_state WHERE singleton FOR UPDATE NOWAIT",
    )
    .fetch_optional(&mut *tx)
    .await;
    match rollout {
        Ok(rollout) => Ok((tx, rollout)),
        Err(error) if lock_not_available(&error) => {
            tx.rollback().await?;
            Err(AiUsageError::RolloutBusy)
        }
        Err(error) => {
            let _rollback = tx.rollback().await;
            Err(AiUsageError::Database(error))
        }
    }
}

/// `FOR UPDATE NOWAIT` refuses a contended row with SQLSTATE 55P03 instead of
/// waiting for it.
fn lock_not_available(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(error) if error.code().as_deref() == Some("55P03"))
}

pub async fn enable_ai_hard_caps(
    pool: &PgPool,
    operator: UserId,
    quiet_window_seconds: u64,
) -> Result<(), AiUsageError> {
    let seconds = i64::try_from(quiet_window_seconds)
        .ok()
        .filter(|seconds| (1..=2_592_000).contains(seconds))
        .ok_or(AiUsageError::InvalidResolution)?;
    let (mut tx, rollout) = begin_rollout_transition(pool).await?;
    let rollout =
        rollout.ok_or_else(|| AiUsageError::HardCapsGate("rollout state is missing".to_owned()))?;
    let last_legacy: Option<chrono::DateTime<chrono::Utc>> =
        rollout.get("last_legacy_ledger_insert_at");
    if last_legacy
        .is_some_and(|inserted| inserted > chrono::Utc::now() - chrono::Duration::seconds(seconds))
    {
        return Err(AiUsageError::HardCapsGate(
            "an N-1 ledger insert occurred within the quiet window".to_owned(),
        ));
    }
    let backfill_complete: bool = sqlx::query_scalar(
        "SELECT completed_at IS NOT NULL FROM ai_usage_backfill_state WHERE singleton",
    )
    .fetch_one(&mut *tx)
    .await?;
    let unprocessed_rows: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ai_usage_ledger l
         WHERE l.reservation_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM ai_usage_legacy_backfill b WHERE b.ledger_id = l.id))",
    )
    .fetch_one(&mut *tx)
    .await?;
    if !backfill_complete || unprocessed_rows {
        return Err(AiUsageError::HardCapsGate(
            "the historical usage backfill is incomplete".to_owned(),
        ));
    }
    let legacy_jobs: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM jobs
         WHERE jobs_workload_class_for_kind(kind) = 'ai'
           AND kind NOT IN ('ai_usage_reconcile', 'ai_usage_export_replay')
           AND state IN ('queued', 'running'))",
    )
    .fetch_one(&mut *tx)
    .await?;
    if legacy_jobs {
        return Err(AiUsageError::HardCapsGate(
            "an AI job is queued or running".to_owned(),
        ));
    }
    sqlx::query(
        "UPDATE ai_usage_rollout_state
         SET ai_hard_caps_enforced = true, hard_caps_enabled_at = now(), updated_at = now()
         WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata)
         VALUES ($1, 'ai.hard_caps_enabled', 'user', $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(serde_json::json!({"quiet_window_seconds": quiet_window_seconds}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn disable_ai_hard_caps(pool: &PgPool, operator: UserId) -> Result<(), AiUsageError> {
    let (mut tx, rollout) = begin_rollout_transition(pool).await?;
    rollout.ok_or_else(|| AiUsageError::HardCapsGate("rollout state is missing".to_owned()))?;
    sqlx::query(
        "UPDATE ai_usage_rollout_state
         SET ai_hard_caps_enforced = false, hard_caps_disabled_at = now(), updated_at = now()
         WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    // Suspending admission must not suspend recovery of existing liabilities.
    // The database capability fence protects recovery jobs from N-1 claims.
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata)
         VALUES ($1, 'ai.hard_caps_disabled', 'user', $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(serde_json::json!({"parked_jobs": 0}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Durable admission and export rollout gates. Liability recovery is
/// independent; database capability fencing protects its jobs during rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiUsageRolloutGates {
    /// Admission enforcement only; liability recovery remains available.
    pub hard_caps_enforced: bool,
    /// `ai_usage_export_replay` is scheduled only while this is set.
    pub export_activated: bool,
}

pub async fn ai_usage_rollout_gates(pool: &PgPool) -> Result<AiUsageRolloutGates, AiUsageError> {
    let row = sqlx::query(
        "SELECT ai_hard_caps_enforced, ai_usage_export_activated
         FROM ai_usage_rollout_state WHERE singleton",
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AiUsageError::Reconciliation)?;
    Ok(AiUsageRolloutGates {
        hard_caps_enforced: row.get("ai_hard_caps_enforced"),
        export_activated: row.get("ai_usage_export_activated"),
    })
}

/// Activate the usage export rollout gate. `ai_usage_export_replay` jobs are
/// scheduled only while this is set. The database independently fences claims
/// by decoder declaration, including N-1 workers with absent/stale heartbeats.
/// The quiet window checks legacy AI writers, not worker membership.
pub async fn enable_ai_usage_export(
    pool: &PgPool,
    operator: UserId,
    quiet_window_seconds: u64,
) -> Result<(), AiUsageError> {
    let seconds = i64::try_from(quiet_window_seconds)
        .ok()
        .filter(|seconds| (1..=2_592_000).contains(seconds))
        .ok_or(AiUsageError::InvalidResolution)?;
    let (mut tx, rollout) = begin_rollout_transition(pool).await?;
    rollout.ok_or_else(|| AiUsageError::UsageExportGate("rollout state is missing".to_owned()))?;
    let last_legacy: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT last_legacy_ledger_insert_at FROM ai_usage_rollout_state WHERE singleton",
    )
    .fetch_one(&mut *tx)
    .await?;
    if last_legacy
        .is_some_and(|inserted| inserted > chrono::Utc::now() - chrono::Duration::seconds(seconds))
    {
        return Err(AiUsageError::UsageExportGate(
            "an N-1 ledger insert occurred within the quiet window".to_owned(),
        ));
    }
    sqlx::query(
        "UPDATE ai_usage_rollout_state
         SET ai_usage_export_activated = true, ai_usage_export_activated_at = now(), updated_at = now()
         WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata)
         VALUES ($1, 'ai.usage_export_enabled', 'user', $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(serde_json::json!({"quiet_window_seconds": quiet_window_seconds}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Deactivate the usage export rollout gate before a rollback. Scheduling
/// stops and every queued replay job is parked in a state no claim accepts, so
/// an N-1 binary can never claim and dead-letter the kind. Rollback requires
/// this command first.
///
/// Like every rollout transition it never waits for the job-write guard or the
/// rollout row: contention returns [`AiUsageError::RolloutBusy`] with every
/// lock released, and the operator retries until the command reports success.
/// A successful command is final: writers before it are committed and parked
/// by this pass, writers after it park themselves against the closed gate.
pub async fn disable_ai_usage_export(pool: &PgPool, operator: UserId) -> Result<u64, AiUsageError> {
    let (mut tx, rollout) = begin_rollout_transition(pool).await?;
    rollout.ok_or_else(|| AiUsageError::UsageExportGate("rollout state is missing".to_owned()))?;
    sqlx::query(
        "UPDATE ai_usage_rollout_state
         SET ai_usage_export_activated = false, ai_usage_export_deactivated_at = now(),
             updated_at = now()
         WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    let parked_jobs = park_queued_jobs(
        &mut tx,
        &JobKind::AiUsageExportReplay,
        "parked by ai-usage disable-export",
    )
    .await?;
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata)
         VALUES ($1, 'ai.usage_export_disabled', 'user', $2, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(serde_json::json!({"parked_jobs": parked_jobs}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(parked_jobs)
}

pub async fn resolve_ai_usage_reservation(
    pool: &PgPool,
    reservation_id: Uuid,
    operator: UserId,
    resolution: AiUsageResolution,
) -> Result<(), AiUsageError> {
    match &resolution {
        AiUsageResolution::Settle { actual_cost_micros } if *actual_cost_micros < 0 => {
            return Err(AiUsageError::InvalidResolution);
        }
        AiUsageResolution::Void { reason } if reason.trim().is_empty() || reason.len() > 500 => {
            return Err(AiUsageError::InvalidResolution);
        }
        _ => {}
    }
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
                "SELECT workspace_id, project_id, run_id, run_budget_id, period_start,
                        reserved_cost_micros, recognized_cost_micros, overage_micros, state, price_version,
                observed_input_tokens, observed_output_tokens, observed_reasoning_tokens,
                observed_commercial_units, provider_request_id, provider_response_id,
                input_micros_per_million_tokens, output_micros_per_million_tokens,
                reasoning_micros_per_million_tokens
         FROM ai_usage_reservations WHERE id = $1 FOR UPDATE",
    )
    .bind(reservation_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AiUsageError::ReservationNotResolvable)?;
    let state: String = row.get("state");
    if matches!(state.as_str(), "settled" | "voided") {
        return Err(AiUsageError::ReservationNotResolvable);
    }
    let workspace_id: Uuid = row.get("workspace_id");
    let project_id: Option<Uuid> = row.get("project_id");
    let run_budget_id: Uuid = row.get("run_budget_id");
    let period_start: chrono::NaiveDate = row.get("period_start");
    lock_project_bucket_lifecycle(&mut tx, project_id)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    let reserved: i64 = row.get("reserved_cost_micros");
    let recognized: i64 = row.get("recognized_cost_micros");
    let prior_overage: i64 = row.get("overage_micros");
    lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    lock_bucket(&mut tx, "workspace", workspace_id, period_start)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    lock_workspace_admission(&mut tx, workspace_id)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    if let Some(project_id) = project_id {
        lock_bucket(&mut tx, "project", project_id, period_start)
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
    }
    lock_bucket(&mut tx, "run", run_budget_id, RUN_BUCKET_PERIOD)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;

    let (action, actual_cost, reason) = match resolution {
        AiUsageResolution::Settle { actual_cost_micros } => {
            let remaining_hold = reserved.saturating_sub(recognized).max(0);
            let spent_delta = actual_cost_micros
                .checked_sub(recognized)
                .ok_or(AiUsageError::InvalidResolution)?;
            let held_delta = remaining_hold
                .checked_neg()
                .ok_or(AiUsageError::InvalidResolution)?;
            for (kind, id) in usage_scope_ids(workspace_id, project_id, run_budget_id) {
                adjust_bucket(
                    &mut tx,
                    kind,
                    id,
                    if kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    spent_delta,
                    held_delta,
                )
                .await
                .map_err(|_| AiUsageError::Reconciliation)?;
            }
            let overage = actual_cost_micros.saturating_sub(reserved).max(0);
            let new_overage = overage.saturating_sub(prior_overage).max(0);
            if new_overage > 0 {
                block_workspace(&mut tx, workspace_id, reservation_id, new_overage)
                    .await
                    .map_err(|_| AiUsageError::Reconciliation)?;
            }
            let evidence = StoredUsageEvidence {
                input_tokens: row.get("observed_input_tokens"),
                output_tokens: row.get("observed_output_tokens"),
                reasoning_tokens: row.get("observed_reasoning_tokens"),
                commercial_units: row.get("observed_commercial_units"),
                provider_request_id: row.get("provider_request_id"),
                provider_response_id: row.get("provider_response_id"),
            };
            sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = 'settled', actual_cost_micros = $2, overage_micros = $3,
                     current_price_version = 'operator-resolution', pricing_decision = 'operator_resolved',
                     resolved_by_user_id = $4, resolution_reason = 'operator_settle', settled_at = now()
                 WHERE id = $1",
            )
            .bind(reservation_id)
            .bind(actual_cost_micros)
            .bind(overage)
            .bind(operator.as_uuid())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO ai_usage_ledger
                 (id, project_id, workspace_id, principal_kind, principal_id, run_id, run_budget_id,
                  step_id, profile, provider, model, purpose, input_tokens, output_tokens,
                  reasoning_tokens, cost_micros, commercial_units, price_version,
                  current_price_version, pricing_decision, period_start, reservation_id,
                  provider_request_id, provider_response_id)
                 SELECT $1, project_id, workspace_id, principal_kind, principal_id, run_id, run_budget_id,
                        step_id, profile, provider, model, purpose, $2, $3, $4, $5, $6,
                        price_version, 'operator-resolution', 'operator_resolved', period_start, id, $7, $8
                 FROM ai_usage_reservations WHERE id = $1",
            )
            .bind(reservation_id)
            .bind(evidence.input_tokens)
            .bind(evidence.output_tokens)
            .bind(evidence.reasoning_tokens)
            .bind(actual_cost_micros)
            .bind(evidence.commercial_units)
            .bind(evidence.provider_request_id)
            .bind(evidence.provider_response_id)
            .execute(&mut *tx)
            .await?;
            ("settle", Some(actual_cost_micros), None)
        }
        AiUsageResolution::Void { reason } => {
            let has_usage = row.get::<Option<i64>, _>("observed_input_tokens").is_some()
                || row
                    .get::<Option<i64>, _>("observed_output_tokens")
                    .is_some()
                || row
                    .get::<Option<i64>, _>("observed_reasoning_tokens")
                    .is_some()
                || recognized > 0;
            if has_usage {
                return Err(AiUsageError::InvalidResolution);
            }
            let held = reserved.saturating_sub(recognized).max(0);
            let held_delta = held.checked_neg().ok_or(AiUsageError::InvalidResolution)?;
            for (kind, id) in usage_scope_ids(workspace_id, project_id, run_budget_id) {
                adjust_bucket(
                    &mut tx,
                    kind,
                    id,
                    if kind == "run" {
                        RUN_BUCKET_PERIOD
                    } else {
                        period_start
                    },
                    0,
                    held_delta,
                )
                .await
                .map_err(|_| AiUsageError::Reconciliation)?;
            }
            sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = 'voided', void_reason = $2, resolved_by_user_id = $3,
                     resolution_reason = $2, settled_at = now()
                 WHERE id = $1",
            )
            .bind(reservation_id)
            .bind(&reason)
            .bind(operator.as_uuid())
            .execute(&mut *tx)
            .await?;
            ("void", None, Some(reason))
        }
    };
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata, workspace_id, project_id)
         VALUES ($1, 'ai.usage_reservation_resolved', 'user', $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(match (action, actual_cost, reason.as_deref()) {
        ("settle", Some(cost), _) => serde_json::json!({
            "reservation_id": reservation_id,
            "resolution": action,
            "cost_micros": cost,
        }),
        (_, _, Some(reason)) => serde_json::json!({
            "reservation_id": reservation_id,
            "resolution": action,
            "reason": reason,
        }),
        _ => serde_json::json!({"reservation_id": reservation_id, "resolution": action}),
    })
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *tx)
    .await?;
    if action == "settle" {
        garbage_collect_detached_project(&mut tx, project_id)
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
    } else {
        garbage_collect_detached_project(&mut tx, project_id)
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
    }
    tx.commit().await?;
    Ok(())
}

fn usage_scope_ids(
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    run_budget_id: Uuid,
) -> Vec<(&'static str, Uuid)> {
    let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
    if let Some(project_id) = project_id {
        scopes.push(("project", project_id));
    }
    scopes.push(("run", run_budget_id));
    scopes
}

pub async fn set_ai_usage_cap(
    pool: &PgPool,
    scope: AiUsageLimitScope,
    limit_micros: i64,
    operator: UserId,
) -> Result<(), AiUsageError> {
    if !(0..=100_000_000_000).contains(&limit_micros) {
        return Err(AiUsageError::InvalidBudget);
    }
    let mut tx = pool.begin().await?;
    let period_start: chrono::NaiveDate =
        sqlx::query_scalar("SELECT date_trunc('month', now() AT TIME ZONE 'UTC')::date")
            .fetch_one(&mut *tx)
            .await?;
    let (scope_kind, scope_id, workspace_id, project_id) = match scope {
        AiUsageLimitScope::Platform => ("platform", PLATFORM_USAGE_ID, None, None),
        AiUsageLimitScope::Workspace(workspace_id) => (
            "workspace",
            workspace_id.as_uuid(),
            Some(workspace_id.as_uuid()),
            None,
        ),
        AiUsageLimitScope::Project(project_id) => {
            let workspace = sqlx::query_scalar::<_, Uuid>(
                "SELECT workspace_id FROM projects WHERE id = $1 FOR NO KEY UPDATE",
            )
            .bind(project_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AiUsageError::ProjectNotFound)?;
            (
                "project",
                project_id.as_uuid(),
                Some(workspace),
                Some(project_id.as_uuid()),
            )
        }
    };
    ensure_bucket_from_limit(
        &mut tx,
        "platform",
        PLATFORM_USAGE_ID,
        None,
        None,
        period_start,
    )
    .await
    .map_err(|_| AiUsageError::Reconciliation)?;
    if let Some(workspace_id) = workspace_id {
        ensure_bucket_from_limit(
            &mut tx,
            "workspace",
            workspace_id,
            Some(workspace_id),
            None,
            period_start,
        )
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    }
    if let (Some(workspace_id), Some(project_id)) = (workspace_id, project_id) {
        ensure_bucket_from_limit(
            &mut tx,
            "project",
            project_id,
            Some(workspace_id),
            Some(project_id),
            period_start,
        )
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    }

    // Cap updates use the same platform → workspace → project bucket order
    // as reservations, then read the old cap from the locked target bucket.
    lock_bucket(&mut tx, "platform", PLATFORM_USAGE_ID, period_start)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    if let Some(workspace_id) = workspace_id {
        lock_bucket(&mut tx, "workspace", workspace_id, period_start)
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
    }
    let target_bucket = lock_bucket(&mut tx, scope_kind, scope_id, period_start)
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    if target_bucket.spent_micros > limit_micros
        || target_bucket.reserved_micros > limit_micros - target_bucket.spent_micros
    {
        return Err(AiUsageError::InvalidBudget);
    }
    let old_limit = target_bucket.limit_micros;
    sqlx::query(
        "UPDATE ai_usage_limits SET limit_micros = $3, updated_at = now()
         WHERE scope_kind = $1 AND scope_id = $2",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(limit_micros)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE ai_usage_period_buckets SET limit_micros = $3, updated_at = now()
         WHERE scope_kind = $1 AND scope_id = $2
           AND period_start = date_trunc('month', now() AT TIME ZONE 'UTC')::date",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .bind(limit_micros)
    .execute(&mut *tx)
    .await?;
    if let Some(project_id) = project_id {
        sqlx::query("UPDATE projects SET ai_monthly_budget_micros = $2 WHERE id = $1")
            .bind(project_id)
            .bind(limit_micros)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO ai_usage_limit_changes
         (id, scope_kind, scope_id, workspace_id, project_id, old_limit_micros, new_limit_micros, changed_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(Uuid::new_v4())
    .bind(scope_kind)
    .bind(scope_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(old_limit)
    .bind(limit_micros)
    .bind(operator.as_uuid())
    .execute(&mut *tx)
    .await?;
    let metadata = serde_json::json!({
        "scope_kind": scope_kind,
        "old_limit_micros": old_limit,
        "new_limit_micros": limit_micros,
    });
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata, workspace_id, project_id)
         VALUES ($1, 'ai.usage_limit_changed', 'user', $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(metadata)
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn waive_ai_usage_overage(
    pool: &PgPool,
    workspace_id: WorkspaceId,
    operator: UserId,
) -> Result<(), AiUsageError> {
    let mut tx = pool.begin().await?;
    let blocked: bool = lock_workspace_admission(&mut tx, workspace_id.as_uuid())
        .await
        .map_err(|_| AiUsageError::Reconciliation)?;
    if !blocked {
        return Err(AiUsageError::NoOverageBlock);
    }
    let overage: Option<i64> = sqlx::query_scalar(
        "UPDATE ai_usage_workspace_blocks
         SET cleared_at = now(), cleared_by_user_id = $2, clear_reason = 'operator_waiver'
         WHERE workspace_id = $1 AND cleared_at IS NULL
         RETURNING overage_micros",
    )
    .bind(workspace_id.as_uuid())
    .bind(operator.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    let overage = overage.ok_or(AiUsageError::NoOverageBlock)?;
    sqlx::query(
        "UPDATE ai_usage_workspace_admission_state
         SET blocked = false, updated_at = now() WHERE workspace_id = $1",
    )
    .bind(workspace_id.as_uuid())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO admin_audit_events
         (id, event_type, principal_kind, principal_user_id, metadata, workspace_id)
         VALUES ($1, 'ai.usage_overage_waived', 'user', $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(serde_json::json!({ "overage_micros": overage }))
    .bind(workspace_id.as_uuid())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::warn!(overage_micros = overage, "workspace AI overage waived");
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct AiUsageReconcileSummary {
    pub inspected: u64,
    pub settled_from_observation: u64,
    pub still_unresolved: u64,
}

pub async fn reconcile_ai_usage_workspace(
    pool: &PgPool,
    workspace_id: WorkspaceId,
    current_price_version: &str,
) -> Result<AiUsageReconcileSummary, AiUsageError> {
    let rows = sqlx::query(
        "SELECT r.id, r.period_start, r.reserved_cost_micros, r.price_version,
                r.input_micros_per_million_tokens, r.output_micros_per_million_tokens,
                r.reasoning_micros_per_million_tokens, r.observed_input_tokens,
                r.observed_output_tokens, r.observed_reasoning_tokens,
                r.observed_commercial_units, r.provider_request_id,
                r.provider_response_id
         FROM ai_usage_reservations r
         LEFT JOIN jobs j ON j.id = r.job_id
         WHERE r.workspace_id = $1
           AND r.state = 'observed'
           AND r.observed_input_tokens IS NOT NULL
           AND r.observed_output_tokens IS NOT NULL
           AND (r.reasoning_micros_per_million_tokens IS NULL
                OR r.observed_reasoning_tokens IS NOT NULL)
           AND r.job_id IS NOT NULL
           AND (j.id IS NULL
             OR (j.claim_seq <> r.attempt_fence
                 AND (j.state <> 'running' OR j.lease_owner IS NOT NULL))
             OR (j.state <> 'running' AND j.lease_owner IS NULL AND j.leased_until IS NULL))
         ORDER BY r.opened_at, r.id
         LIMIT 500",
    )
    .bind(workspace_id.as_uuid())
    .fetch_all(pool)
    .await?;
    let still_unresolved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ai_usage_reservations r
         LEFT JOIN jobs j ON j.id = r.job_id
         WHERE r.workspace_id = $1
           AND (r.state IN ('open', 'unresolved') OR
             (r.state = 'observed' AND
               (r.observed_input_tokens IS NULL OR r.observed_output_tokens IS NULL OR
                (r.reasoning_micros_per_million_tokens IS NOT NULL
                 AND r.observed_reasoning_tokens IS NULL))))
           AND r.job_id IS NOT NULL
           AND (j.id IS NULL
             OR (j.claim_seq <> r.attempt_fence
                 AND (j.state <> 'running' OR j.lease_owner IS NOT NULL))
             OR (j.state <> 'running' AND j.lease_owner IS NULL AND j.leased_until IS NULL))",
    )
    .bind(workspace_id.as_uuid())
    .fetch_one(pool)
    .await?;
    let mut summary = AiUsageReconcileSummary {
        inspected: u64::try_from(rows.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(still_unresolved).unwrap_or(0)),
        still_unresolved: u64::try_from(still_unresolved).unwrap_or(u64::MAX),
        ..AiUsageReconcileSummary::default()
    };
    for row in rows {
        let to_tokens = |value: Option<i64>| {
            value
                .map(|value| u64::try_from(value).map_err(|_| AiUsageError::Reconciliation))
                .transpose()
        };
        let observation = UsageObservation {
            input_tokens: to_tokens(row.get("observed_input_tokens"))?,
            output_tokens: to_tokens(row.get("observed_output_tokens"))?,
            reasoning_tokens: to_tokens(row.get("observed_reasoning_tokens"))?,
            commercial_units: row.get("observed_commercial_units"),
            provider_request_id: row.get("provider_request_id"),
            provider_response_id: row.get("provider_response_id"),
        };
        let reservation = UsageReservation {
            id: row.get("id"),
            workspace_id,
            period_start: row.get("period_start"),
            reserved_cost_micros: row.get("reserved_cost_micros"),
            price: ai_usage_price(&row),
        };
        PostgresUsageLedger::new(pool)
            .settle(&reservation, observation, current_price_version)
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
        summary.settled_from_observation += 1;
    }
    let detached_projects_collected = sweep_detached_project_buckets(pool).await?;
    tracing::info!(
        inspected = summary.inspected,
        settled_from_observation = summary.settled_from_observation,
        still_unresolved = summary.still_unresolved,
        detached_projects_collected,
        "workspace AI usage reconciliation completed"
    );
    Ok(summary)
}

const DETACHED_BUCKET_SWEEP_BATCH: i64 = 100;

/// Periodic recovery for detached projects whose last liability already
/// became terminal. Each candidate is rechecked under the same advisory lock
/// used by project deletion and final settlement before its buckets are removed.
async fn sweep_detached_project_buckets(pool: &PgPool) -> Result<u64, AiUsageError> {
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT d.project_id FROM ai_usage_detached_projects d
         WHERE NOT EXISTS (
           SELECT 1 FROM ai_usage_reservations r
           WHERE r.project_id = d.project_id AND r.state IN ('open', 'observed', 'unresolved'))
         ORDER BY d.project_id
         LIMIT $1",
    )
    .bind(DETACHED_BUCKET_SWEEP_BATCH)
    .fetch_all(pool)
    .await?;
    let mut collected = 0_u64;
    for project_id in candidates {
        let mut tx = pool.begin().await?;
        lock_project_bucket_lifecycle(&mut tx, Some(project_id))
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
        garbage_collect_detached_project(&mut tx, Some(project_id))
            .await
            .map_err(|_| AiUsageError::Reconciliation)?;
        let still_detached: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM ai_usage_detached_projects WHERE project_id = $1)",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if !still_detached {
            collected = collected.saturating_add(1);
        }
    }
    Ok(collected)
}

const ALL_PROFILES: [ModelProfile; 5] = [
    ModelProfile::FastClassifier,
    ModelProfile::Embedding,
    ModelProfile::Reasoning,
    ModelProfile::LongContextReasoning,
    ModelProfile::PremiumSynthesis,
];

/// Provider labels that AI routes have been stored under. A route still
/// carrying one of them after the configured provider changed is re-pointed.
const AI_PROVIDER_LABELS: [&str; 2] = ["opencode-go", "zai"];

/// What [`ensure_default_model_routes`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultRoutes {
    /// Default routes created because a profile had no enabled route.
    pub created: u64,
    /// Enabled routes re-pointed from another DeepRef provider label.
    pub repointed: u64,
}

/// Makes the default routes follow the configured provider.
///
/// An enabled route in effect now that carries another DeepRef provider label
/// (`zai` after the move to OpenCode Go, or the reverse) is re-pointed to
/// `provider`. The label is the adapter key and part of the model identity, so
/// a route kept under the old label would call the wrong endpoint and be
/// calibrated as the wrong model. Historical rows (past runs, ledger entries,
/// disabled or expired routes, calibration bundles) are never rewritten, and
/// neither are custom routes under other providers. Each chat profile without
/// an enabled route gets a default one. The embedding profile is skipped: chat
/// models cannot embed.
pub async fn ensure_default_model_routes(
    pool: &PgPool,
    provider: &str,
    model: &str,
) -> Result<DefaultRoutes, sqlx::Error> {
    let labels: Vec<String> = AI_PROVIDER_LABELS
        .iter()
        .map(|label| (*label).to_owned())
        .collect();
    let repointed = sqlx::query(
        "UPDATE ai_model_routes AS route SET provider = $1
         WHERE route.enabled AND route.provider = ANY($2) AND route.provider <> $1
           AND route.effective_from <= now()
           AND (route.effective_until IS NULL OR route.effective_until > now())
           AND NOT EXISTS (
             SELECT 1 FROM ai_model_routes AS other
             WHERE other.profile = route.profile AND other.provider = $1
               AND other.model = route.model AND other.model_version = route.model_version
               AND other.effective_from = route.effective_from)",
    )
    .bind(provider)
    .bind(labels)
    .execute(pool)
    .await?
    .rows_affected();
    let mut created = 0;
    for profile in ALL_PROFILES {
        if profile == ModelProfile::Embedding {
            continue;
        }
        let result = sqlx::query(
            "INSERT INTO ai_model_routes (id, profile, provider, model, model_version, parameters)
             SELECT $1, $2, $3, $4, $4, '{\"temperature\":0.0,\"max_tokens\":2048}'::jsonb
             WHERE NOT EXISTS (
               SELECT 1 FROM ai_model_routes
               WHERE profile = $2 AND enabled AND effective_from <= now()
                 AND (effective_until IS NULL OR effective_until > now()))",
        )
        .bind(Uuid::new_v4())
        .bind(profile.as_str())
        .bind(provider)
        .bind(model)
        .execute(pool)
        .await?;
        created += result.rows_affected();
    }
    Ok(DefaultRoutes { created, repointed })
}
