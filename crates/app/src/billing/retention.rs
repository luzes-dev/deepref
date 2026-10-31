//! Five-year financial retention purge (BUSINESS_DECISIONS "Billing and AI
//! usage records"; 08 "Privacy and retention").
//!
//! While retained, a deleted workspace keeps only the financial tombstone
//! documented in docs/dodo-billing.md. Once every billing account naming the
//! workspace is past its `purge_after` (detachment + 5 years), the workspace is
//! gone and the record itself is older than five years, this bounded operator
//! purge deletes it. The database refuses these deletes unless the transaction
//! sets `deepref.billing_purge` and the row is eligible
//! (`billing_guard_financial_delete`, migration 0010). Usage-fact identities in
//! `ai_usage_fact_ids` are kept: they carry no tenant data and keep
//! deduplication sound.
use sqlx::{PgPool, Postgres, Transaction};

pub const DEFAULT_BATCH: i64 = 500;
const MAX_ROUNDS: usize = 32;

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FinancialPurgeReport {
    pub dry_run: bool,
    pub usage_resolutions: u64,
    pub usage_fact_targets: u64,
    pub usage_ledger: u64,
    pub usage_corrections: u64,
    pub usage_reservations: u64,
    pub usage_period_buckets: u64,
    pub billing_accounts: u64,
    /// More eligible rows remain after this bounded run.
    pub remaining: bool,
}

macro_rules! step {
    ($key:literal, $table:literal, $id:literal, $select:literal) => {
        Step {
            key: $key,
            delete: concat!(
                "DELETE FROM ",
                $table,
                " WHERE ",
                $id,
                " IN (",
                $select,
                ")"
            ),
            count: concat!("SELECT count(*) FROM (", $select, ") s"),
        }
    };
}

struct Step {
    key: &'static str,
    delete: &'static str,
    count: &'static str,
}

/// Each statement selects at most `$1` eligible rows. Order matters: targets
/// reference accounts, and a correction becomes eligible once its ledger row
/// is gone.
const STEPS: &[Step] = &[
    step!(
        "usage_resolutions",
        "billing_usage_resolutions",
        "fact_id",
        "SELECT fact_id FROM billing_usage_resolutions
         WHERE resolved_at < now()-interval '5 years'
           AND billing_retention_expired(workspace_id,resolved_at)
         ORDER BY resolved_at,fact_id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
    step!(
        "usage_fact_targets",
        "billing_usage_fact_targets",
        "fact_id",
        "SELECT fact_id FROM billing_usage_fact_targets
         WHERE bound_at < now()-interval '5 years'
           AND billing_retention_expired(workspace_id,bound_at)
         ORDER BY bound_at,fact_id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
    step!(
        "usage_ledger",
        "ai_usage_ledger",
        "id",
        "SELECT id FROM ai_usage_ledger
         WHERE created_at < now()-interval '5 years'
           AND billing_retention_expired(workspace_id,created_at)
         ORDER BY created_at,id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
    step!(
        "usage_corrections",
        "ai_usage_corrections",
        "id",
        "SELECT c.id FROM ai_usage_corrections c
         WHERE c.created_at < now()-interval '5 years'
           AND NOT EXISTS (SELECT 1 FROM ai_usage_ledger l WHERE l.id=c.ledger_id)
         ORDER BY c.created_at,c.id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
    step!(
        "usage_reservations",
        "ai_usage_reservations",
        "id",
        "SELECT id FROM ai_usage_reservations
         WHERE state IN ('settled','voided')
           AND coalesce(settled_at,opened_at) < now()-interval '5 years'
           AND billing_retention_expired(workspace_id,coalesce(settled_at,opened_at))
         ORDER BY opened_at,id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
    // Period buckets are aggregates, not guarded facts; they go once their
    // workspace's records are past retention.
    step!(
        "usage_period_buckets",
        "ai_usage_period_buckets",
        "(scope_kind,scope_id,period_start)",
        "SELECT scope_kind,scope_id,period_start FROM ai_usage_period_buckets
         WHERE scope_kind<>'platform' AND workspace_id IS NOT NULL
           AND period_start < (now()-interval '5 years')::date
           AND billing_retention_expired(workspace_id,period_start::timestamptz)
         ORDER BY period_start,scope_kind,scope_id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
    step!(
        "billing_accounts",
        "billing_accounts",
        "id",
        "SELECT a.id FROM billing_accounts a
         WHERE a.provisioning='detached' AND a.purge_after < now()
           AND NOT EXISTS (SELECT 1 FROM billing_usage_fact_targets t WHERE t.account_id=a.id)
         ORDER BY a.purge_after,a.id LIMIT $1 FOR UPDATE SKIP LOCKED"
    ),
];

fn slot<'a>(report: &'a mut FinancialPurgeReport, key: &str) -> &'a mut u64 {
    match key {
        "usage_resolutions" => &mut report.usage_resolutions,
        "usage_fact_targets" => &mut report.usage_fact_targets,
        "usage_ledger" => &mut report.usage_ledger,
        "usage_corrections" => &mut report.usage_corrections,
        "usage_reservations" => &mut report.usage_reservations,
        "usage_period_buckets" => &mut report.usage_period_buckets,
        _ => &mut report.billing_accounts,
    }
}

async fn gated(pool: &PgPool) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('deepref.billing_purge','on',true)")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

/// Bounded purge: at most `MAX_ROUNDS` committed batches of `batch` rows per
/// table. With `dry_run` it counts one batch per table and deletes nothing.
pub async fn purge_financial_retention(
    pool: &PgPool,
    dry_run: bool,
    batch: Option<i64>,
) -> Result<FinancialPurgeReport, sqlx::Error> {
    let batch = batch.unwrap_or(DEFAULT_BATCH).clamp(1, 1_000);
    let mut report = FinancialPurgeReport {
        dry_run,
        ..Default::default()
    };
    for step in STEPS {
        if dry_run {
            let mut tx = pool.begin().await?;
            let count: i64 = sqlx::query_scalar(step.count)
                .bind(batch)
                .fetch_one(&mut *tx)
                .await?;
            tx.rollback().await?;
            *slot(&mut report, step.key) = u64::try_from(count).unwrap_or(0);
            report.remaining |= count == batch;
            continue;
        }
        for round in 0..MAX_ROUNDS {
            let mut tx = gated(pool).await?;
            let deleted = sqlx::query(step.delete)
                .bind(batch)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            tx.commit().await?;
            *slot(&mut report, step.key) += deleted;
            if deleted < u64::try_from(batch).unwrap_or(0) {
                break;
            }
            if round + 1 == MAX_ROUNDS {
                report.remaining = true;
            }
        }
    }
    Ok(report)
}
