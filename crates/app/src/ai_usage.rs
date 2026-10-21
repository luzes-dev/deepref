use deepref_ai::{
    AiError, AiFuture, BudgetSnapshot, ModelProfile, UsageLedger, UsageObservation,
    UsagePriceSnapshot, UsageReservation, UsageReservationRequest, UsageTopUpRequest,
    ZeroLiabilityProof,
};
use deepref_domain::{ProjectId, UserId, WorkspaceId};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

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
            let reservation_id = Uuid::new_v4();
            let period_start: chrono::NaiveDate =
                sqlx::query_scalar("SELECT date_trunc('month', now() AT TIME ZONE 'UTC')::date")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|_| db_failure())?;
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM ai_usage_workspace_blocks WHERE workspace_id = $1 AND cleared_at IS NULL)",
            )
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            if blocked {
                tracing::warn!(
                    reason = "workspace_overage_block",
                    "AI usage reservation denied"
                );
                return Err(AiError::WorkspaceAiBlocked);
            }

            // Capture the current configured caps, then initialize the period
            // buckets. Row updates below always take locks platform → workspace
            // → project → run and are rolled back as a unit on any denial.
            let platform_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM ai_usage_limits WHERE scope_kind = 'platform' AND scope_id = $1)",
            )
            .bind(PLATFORM_USAGE_ID)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            let workspace_limit: Option<i64> = sqlx::query_scalar(
                "SELECT limit_micros FROM ai_usage_limits WHERE scope_kind = 'workspace' AND scope_id = $1",
            )
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            if !platform_exists || workspace_limit.is_none() {
                return Err(AiError::MissingUsageWorkspace);
            }
            sqlx::query(
                "INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros)
                 SELECT scope_kind, scope_id, $2, limit_micros FROM ai_usage_limits
                 WHERE scope_kind = 'platform' AND scope_id = $1
                 ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
                   SET limit_micros = EXCLUDED.limit_micros, updated_at = now()",
            )
            .bind(PLATFORM_USAGE_ID)
            .bind(period_start)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            sqlx::query(
                "INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros)
                 SELECT 'workspace', scope_id, scope_id, $2, limit_micros FROM ai_usage_limits
                 WHERE scope_kind = 'workspace' AND scope_id = $1
                 ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
                   SET limit_micros = EXCLUDED.limit_micros, updated_at = now()",
            )
            .bind(workspace_id)
            .bind(period_start)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            if let Some(project_id) = project_id {
                sqlx::query(
                    "INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
                     SELECT 'project', scope_id, $2, scope_id, $3, limit_micros FROM ai_usage_limits
                     WHERE scope_kind = 'project' AND scope_id = $1
                     ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
                       SET limit_micros = EXCLUDED.limit_micros, updated_at = now()",
                )
                .bind(project_id)
                .bind(workspace_id)
                .bind(period_start)
                .execute(&mut *tx)
                .await
                .map_err(|_| db_failure())?;
            }
            sqlx::query(
                "INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, project_id, run_id, period_start, limit_micros)
                 VALUES ('run', $1, $2, $3, $1, $4, $5)
                 ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
                   SET limit_micros = LEAST(ai_usage_period_buckets.limit_micros, EXCLUDED.limit_micros),
                       updated_at = now()",
            )
            .bind(request.attribution.run_id)
            .bind(workspace_id)
            .bind(project_id)
            .bind(period_start)
            .bind(run_limit)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;

            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", request.attribution.run_id));
            for (scope_kind, scope_id) in scopes {
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
                .bind(reserved_cost)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| db_failure())?;
                if available.is_none() {
                    tracing::warn!(
                        scope_kind,
                        reserved_cost_micros = reserved_cost,
                        "AI usage reservation denied"
                    );
                    return Err(AiError::UsageLimitExceeded);
                }
            }

            let (principal_kind, principal_id) = request
                .attribution
                .principal
                .as_ref()
                .map(|principal| (Some(principal.kind()), Some(principal.id())))
                .unwrap_or((None, None));
            let inserted = sqlx::query(
                "INSERT INTO ai_usage_reservations
                 (id, workspace_id, project_id, principal_kind, principal_id, run_id, step_id,
                  job_id, attempt_fence, profile, provider, model, purpose, input_token_bound,
                  max_output_tokens, max_reasoning_tokens, input_micros_per_million_tokens,
                  output_micros_per_million_tokens, reasoning_micros_per_million_tokens,
                  price_version, reserved_cost_micros, period_start)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)",
            )
            .bind(reservation_id)
            .bind(workspace_id)
            .bind(project_id)
            .bind(principal_kind)
            .bind(principal_id)
            .bind(request.attribution.run_id)
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
                "SELECT workspace_id, project_id, run_id, period_start, state,
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
            let run_id: Uuid = row.get("run_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_id));
            for (scope_kind, scope_id) in scopes {
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
                .bind(additional_cost)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| db_failure())?;
                if available.is_none() {
                    tracing::warn!(
                        scope_kind,
                        additional_cost_micros = additional_cost,
                        "AI usage top-up denied"
                    );
                    return Err(AiError::UsageLimitExceeded);
                }
            }
            let updated = sqlx::query(
                "UPDATE ai_usage_reservations
                 SET reserved_cost_micros = $2, input_token_bound = $3,
                     max_output_tokens = $4, max_reasoning_tokens = $5
                 WHERE id = $1 AND state IN ('open', 'observed')",
            )
            .bind(reservation.id)
            .bind(new_reserved)
            .bind(input_bound)
            .bind(max_output)
            .bind(max_reasoning)
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
            let changed = sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = CASE WHEN state = 'open' THEN 'observed' ELSE state END,
                     observed_input_tokens = $2, observed_output_tokens = $3,
                     observed_reasoning_tokens = $4, observed_commercial_units = $5
                 WHERE id = $1 AND state IN ('open', 'observed', 'unresolved')",
            )
            .bind(reservation.id)
            .bind(to_i64(observation.input_tokens)?)
            .bind(to_i64(observation.output_tokens)?)
            .bind(to_i64(observation.reasoning_tokens)?)
            .bind(observation.commercial_units)
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
                "SELECT workspace_id, project_id, run_id, period_start, reserved_cost_micros,
                        recognized_cost_micros, state, price_version,
                        input_micros_per_million_tokens, output_micros_per_million_tokens,
                        reasoning_micros_per_million_tokens, provider, model
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
            let held_cost = reserved_cost.saturating_sub(recognized_cost);
            let workspace_id: Uuid = row.get("workspace_id");
            let project_id: Option<Uuid> = row.get("project_id");
            let run_id: Uuid = row.get("run_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            let spent_delta = actual
                .checked_sub(recognized_cost)
                .ok_or(AiError::InvalidUsageBound)?;
            let held_delta = held_cost.checked_neg().ok_or(AiError::InvalidUsageBound)?;
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_id));
            for (scope_kind, scope_id) in scopes {
                adjust_bucket(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    period_start,
                    spent_delta,
                    held_delta,
                )
                .await?;
            }
            let overage = actual.saturating_sub(reserved_cost).max(0);
            let hierarchy_overage: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(GREATEST(0, spent_micros + reserved_micros - limit_micros)), 0)
                 FROM ai_usage_period_buckets
                 WHERE period_start = $1 AND
                   ((scope_kind = 'platform' AND scope_id = $2)
                    OR (scope_kind = 'workspace' AND scope_id = $3)
                    OR (scope_kind = 'project' AND scope_id = $4)
                    OR (scope_kind = 'run' AND scope_id = $5))",
            )
            .bind(period_start)
            .bind(PLATFORM_USAGE_ID)
            .bind(workspace_id)
            .bind(project_id)
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            let block_amount = overage.max(hierarchy_overage);
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
                     actual_cost_micros = $6, overage_micros = $7,
                     current_price_version = $8, pricing_decision = $9, settled_at = now()
                 WHERE id = $1",
            )
            .bind(reservation.id)
            .bind(to_i64(observation.input_tokens)?)
            .bind(to_i64(observation.output_tokens)?)
            .bind(to_i64(observation.reasoning_tokens)?)
            .bind(observation.commercial_units)
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
                 (id, project_id, workspace_id, principal_kind, principal_id, run_id, step_id,
                  profile, provider, model, purpose, input_tokens, output_tokens, reasoning_tokens,
                  cost_micros, commercial_units, price_version, current_price_version,
                  pricing_decision, period_start, reservation_id)
                 SELECT $1, project_id, workspace_id, principal_kind, principal_id, run_id, step_id,
                        profile, provider, model, purpose, $2, $3, $4, $5, $6, price_version,
                        current_price_version, pricing_decision, period_start, id
                 FROM ai_usage_reservations WHERE id = $1",
            )
            .bind(fact_id)
            .bind(to_i64(observation.input_tokens)?)
            .bind(to_i64(observation.output_tokens)?)
            .bind(to_i64(observation.reasoning_tokens)?)
            .bind(actual)
            .bind(observation.commercial_units)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            let payload = serde_json::json!({
                "reservation_id": reservation.id,
                "workspace_id": workspace_id,
                "project_id": project_id,
                "period_start": period_start,
                "provider": row.get::<String, _>("provider"),
                "model": row.get::<String, _>("model"),
                "cost_micros": actual,
                "commercial_units": observation.commercial_units,
                "price_version": price.version,
                "current_price_version": current_version,
                "pricing_decision": pricing_decision,
            });
            sqlx::query(
                "INSERT INTO ai_usage_outbox (id, reservation_id, event_type, payload)
                 VALUES ($1, $2, 'ai.usage.settled', $3)",
            )
            .bind(Uuid::new_v4())
            .bind(reservation.id)
            .bind(payload)
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
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
                "SELECT workspace_id, project_id, run_id, period_start, reserved_cost_micros,
                        recognized_cost_micros, state, price_version,
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
            if matches!(state.as_str(), "settled" | "voided") {
                return Err(db_failure());
            }
            let prior_recognized: i64 = row.get("recognized_cost_micros");
            let reserved_cost: i64 = row.get("reserved_cost_micros");
            let workspace_id: Uuid = row.get("workspace_id");
            let project_id: Option<Uuid> = row.get("project_id");
            let run_id: Uuid = row.get("run_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            let known_cost = if let Some(observation) = observation.as_ref() {
                ai_usage_price(&row).actual_cost_micros(observation)?
            } else {
                prior_recognized
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
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_id));
            for (scope_kind, scope_id) in scopes {
                adjust_bucket(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    period_start,
                    spent_delta,
                    held_delta,
                )
                .await?;
            }
            let known_overage = known_cost.saturating_sub(reserved_cost).max(0);
            if known_overage > 0 {
                block_workspace(&mut tx, workspace_id, reservation.id, known_overage).await?;
            }
            let (input, output, reasoning, units) = if let Some(usage) = observation.as_ref() {
                (
                    Some(to_i64(usage.input_tokens)?),
                    Some(to_i64(usage.output_tokens)?),
                    Some(to_i64(usage.reasoning_tokens)?),
                    usage.commercial_units,
                )
            } else {
                (None, None, None, None)
            };
            sqlx::query(
                "UPDATE ai_usage_reservations
                 SET state = 'unresolved', unresolved_reason = $2,
                     observed_input_tokens = COALESCE($3, observed_input_tokens),
                     observed_output_tokens = COALESCE($4, observed_output_tokens),
                     observed_reasoning_tokens = COALESCE($5, observed_reasoning_tokens),
                     observed_commercial_units = COALESCE($6, observed_commercial_units),
                     recognized_cost_micros = $7,
                     overage_micros = GREATEST(overage_micros, $8)
                 WHERE id = $1",
            )
            .bind(reservation.id)
            .bind(reason)
            .bind(input)
            .bind(output)
            .bind(reasoning)
            .bind(units)
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
            let ZeroLiabilityProof::ProviderRejected { status } = proof;
            let mut tx = self.pool.begin().await.map_err(|_| db_failure())?;
            let row = sqlx::query(
                "SELECT workspace_id, project_id, run_id, period_start, reserved_cost_micros,
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
            let run_id: Uuid = row.get("run_id");
            let period_start: chrono::NaiveDate = row.get("period_start");
            let reserved_cost: i64 = row.get("reserved_cost_micros");
            let reserved_delta = reserved_cost
                .checked_neg()
                .ok_or(AiError::InvalidUsageBound)?;
            let mut scopes = vec![("platform", PLATFORM_USAGE_ID), ("workspace", workspace_id)];
            if let Some(project_id) = project_id {
                scopes.push(("project", project_id));
            }
            scopes.push(("run", run_id));
            for (scope_kind, scope_id) in scopes {
                adjust_bucket(
                    &mut tx,
                    scope_kind,
                    scope_id,
                    period_start,
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
            .bind(format!("provider_rejected_http_{status}"))
            .execute(&mut *tx)
            .await
            .map_err(|_| db_failure())?;
            tx.commit().await.map_err(|_| db_failure())?;
            tracing::info!(status, "zero-liability AI reservation voided");
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

pub async fn set_ai_usage_cap(
    pool: &PgPool,
    scope: AiUsageLimitScope,
    limit_micros: i64,
    operator: UserId,
) -> Result<(), AiUsageError> {
    if !(0..=100_000_000_000).contains(&limit_micros) {
        return Err(AiUsageError::InvalidBudget);
    }
    let (scope_kind, scope_id, workspace_id, project_id) = match scope {
        AiUsageLimitScope::Platform => ("platform", PLATFORM_USAGE_ID, None, None),
        AiUsageLimitScope::Workspace(workspace_id) => (
            "workspace",
            workspace_id.as_uuid(),
            Some(workspace_id.as_uuid()),
            None,
        ),
        AiUsageLimitScope::Project(project_id) => {
            let workspace =
                sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM projects WHERE id = $1")
                    .bind(project_id.as_uuid())
                    .fetch_optional(pool)
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
    let mut tx = pool.begin().await?;
    let old_limit: Option<i64> = sqlx::query_scalar(
        "SELECT limit_micros FROM ai_usage_limits WHERE scope_kind = $1 AND scope_id = $2 FOR UPDATE",
    )
    .bind(scope_kind)
    .bind(scope_id)
    .fetch_optional(&mut *tx)
    .await?;
    let old_limit = old_limit.ok_or(AiUsageError::WorkspaceNotFound)?;
    if let Some(project_id) = project_id {
        sqlx::query("UPDATE projects SET ai_monthly_budget_micros = $2 WHERE id = $1")
            .bind(project_id)
            .bind(limit_micros)
            .execute(&mut *tx)
            .await?;
    }
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
                r.observed_commercial_units, r.state
         FROM ai_usage_reservations r
         LEFT JOIN jobs j ON j.id = r.job_id
         WHERE r.workspace_id = $1
           AND r.state IN ('open', 'observed', 'unresolved')
           AND r.job_id IS NOT NULL
           AND (j.id IS NULL
             OR (j.claim_seq <> r.attempt_fence
                 AND (j.state <> 'running' OR j.lease_owner IS NOT NULL))
             OR (j.state <> 'running' AND j.lease_owner IS NULL AND j.leased_until IS NULL))
         ORDER BY r.opened_at
         LIMIT 500",
    )
    .bind(workspace_id.as_uuid())
    .fetch_all(pool)
    .await?;
    let mut summary = AiUsageReconcileSummary {
        inspected: rows.len() as u64,
        ..AiUsageReconcileSummary::default()
    };
    for row in rows {
        let state: String = row.get("state");
        let observation = match (
            row.get::<Option<i64>, _>("observed_input_tokens"),
            row.get::<Option<i64>, _>("observed_output_tokens"),
            row.get::<Option<i64>, _>("observed_reasoning_tokens"),
        ) {
            (Some(input), Some(output), Some(reasoning)) => Some(UsageObservation {
                input_tokens: input as u64,
                output_tokens: output as u64,
                reasoning_tokens: reasoning as u64,
                commercial_units: row.get("observed_commercial_units"),
            }),
            _ => None,
        };
        if state == "observed"
            && let Some(observation) = observation
        {
            let reservation = UsageReservation {
                id: row.get("id"),
                period_start: row.get("period_start"),
                reserved_cost_micros: row.get("reserved_cost_micros"),
                price: ai_usage_price(&row),
            };
            PostgresUsageLedger::new(pool)
                .settle(&reservation, observation, current_price_version)
                .await
                .map_err(|_| AiUsageError::Reconciliation)?;
            summary.settled_from_observation += 1;
            continue;
        }
        summary.still_unresolved += 1;
    }
    tracing::info!(
        inspected = summary.inspected,
        settled_from_observation = summary.settled_from_observation,
        still_unresolved = summary.still_unresolved,
        "workspace AI usage reconciliation completed"
    );
    Ok(summary)
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
