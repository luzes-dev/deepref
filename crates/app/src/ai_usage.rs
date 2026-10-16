use deepref_ai::{AiError, AiFuture, BudgetSnapshot, ModelProfile, UsageEntry, UsageLedger};
use deepref_domain::ProjectId;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

/// Default monthly AI budget (US$5.00) in micro-dollars; mirrors the column
/// default in migration 0027.
pub const DEFAULT_AI_MONTHLY_BUDGET_MICROS: i64 = 5_000_000;

#[derive(Debug, Error)]
pub enum AiUsageError {
    #[error("project not found")]
    ProjectNotFound,
    #[error("budget must be between 0 and 100000 USD")]
    InvalidBudget,
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

impl UsageLedger for PostgresUsageLedger {
    fn budget<'a>(&'a self, project_id: ProjectId) -> AiFuture<'a, BudgetSnapshot> {
        Box::pin(async move {
            match get_ai_budget(&self.pool, project_id.as_uuid()).await {
                Ok(snapshot) => Ok(snapshot),
                // A project that vanished mid-flight cannot be billed.
                Err(AiUsageError::ProjectNotFound) => Ok(BudgetSnapshot {
                    budget_micros: i64::MAX,
                    spent_micros: 0,
                }),
                Err(_) => Err(AiError::Persistence("budget lookup failed".to_owned())),
            }
        })
    }

    fn record<'a>(&'a self, entry: UsageEntry) -> AiFuture<'a, ()> {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO ai_usage_ledger
                   (project_id, profile, provider, model, purpose, input_tokens, output_tokens, cost_micros)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            )
            .bind(entry.project_id)
            .bind(&entry.profile)
            .bind(&entry.provider)
            .bind(&entry.model)
            .bind(entry.purpose)
            .bind(i64::try_from(entry.input_tokens).unwrap_or(i64::MAX))
            .bind(i64::try_from(entry.output_tokens).unwrap_or(i64::MAX))
            .bind(entry.cost_micros)
            .execute(&self.pool)
            .await
            .map_err(|_| AiError::Persistence("usage ledger write failed".to_owned()))?;
            Ok(())
        })
    }
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
             SELECT $1, $2, $3, $4, $4, '{\"temperature\":0.0}'::jsonb
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
