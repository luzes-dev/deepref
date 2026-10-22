//! PostgreSQL implementation of AI concurrency admission.
//!
//! The leased-slot table `ai_concurrency_slots` is the pre-`T-VALKEY` mechanism
//! (spec 06) and the conservative fallback whenever Valkey is unavailable
//! (spec 09). Slots are operational capacity only: nothing here touches
//! reservations, settlements or ledger numbers. Advisory locks are not used;
//! they cannot be counted, expired or capacity-bounded.
//!
//! ## Why grants cannot exceed the limit
//!
//! A grant takes exactly one row of the primary key
//! `(scope_kind, scope_key, slot_index)` and only at `slot_index < limit`:
//!
//! - a missing row is created by `INSERT`;
//! - an existing row is taken over only when its committed lease is expired
//!   (`ON CONFLICT DO UPDATE ... WHERE lease_expires_at < clock_timestamp()`,
//!   evaluated on the locked row's latest version);
//! - a live row is never overwritten: the conflicting insert resolves to
//!   nothing and the caller retries another slot or reports saturation.
//!
//! Row writes serialize on the row lock, so each slot row holds at most one
//! live lease at any commit point, and at most `limit` rows are grantable:
//! at most `limit` live leases can exist per scope key under any interleaving.
//! `clock_timestamp()` decides expiry (never `now()`, which is frozen for a
//! transaction). Both keys of a scope are claimed in one transaction in the
//! fixed workspace-then-route order, so a caller never holds one without the
//! other and concurrent callers cannot deadlock.

use std::{sync::Arc, time::Duration};

use deepref_ai::{
    AdmissionError, AdmissionFuture, AiConcurrencyAdmission, AiConcurrencyMetrics,
    AiConcurrencyScope, AiConcurrencyScopeKind, SlotGrant, SlotLease,
};
use sqlx::{PgPool, Row, Transaction};
use uuid::Uuid;

/// Lease duration of one grant. Long calls renew at a third of this cadence.
const DEFAULT_LEASE: Duration = deepref_ai::SLOT_LEASE_TTL;
/// Rows expired longer than this are swept; leases are far shorter, so nothing
/// still owned can be swept out from under its holder.
const SWEEP_RETENTION: Duration = Duration::from_secs(3600);
/// Most rows one sweep deletes.
const SWEEP_BATCH: i64 = 500;
/// A claim that lost the race for its picked slot is retried before the scope
/// is reported saturated. Acquire never waits for a slot; it only re-reads.
const MAX_CLAIM_ATTEMPTS: usize = 3;

const CLAIM_SLOT: &str = "
INSERT INTO ai_concurrency_slots AS s
  (scope_kind, scope_key, slot_index, holder_id, generation, acquired_at, lease_expires_at)
SELECT $1, $2, g.slot_index, $4, nextval('ai_concurrency_slots_generation_seq'),
       clock_timestamp(),
       clock_timestamp() + make_interval(secs => $5::double precision)
FROM generate_series(0, $3::integer - 1) AS g(slot_index)
WHERE NOT EXISTS (
  SELECT 1 FROM ai_concurrency_slots x
  WHERE x.scope_kind = $1 AND x.scope_key = $2 AND x.slot_index = g.slot_index
    AND x.lease_expires_at >= clock_timestamp())
ORDER BY g.slot_index
LIMIT 1
ON CONFLICT (scope_kind, scope_key, slot_index) DO UPDATE
  SET holder_id = EXCLUDED.holder_id,
      generation = nextval('ai_concurrency_slots_generation_seq'),
      acquired_at = clock_timestamp(),
      lease_expires_at = clock_timestamp() + make_interval(secs => $5::double precision)
WHERE s.lease_expires_at < clock_timestamp()
RETURNING s.slot_index, s.generation, s.lease_expires_at, (s.xmax = 0) AS inserted";

const LIVE_SLOTS: &str = "
SELECT count(*)::bigint AS live FROM ai_concurrency_slots
WHERE scope_kind = $1 AND scope_key = $2 AND slot_index < $3
  AND lease_expires_at >= clock_timestamp()";

const RENEW_SLOT: &str = "
UPDATE ai_concurrency_slots
SET lease_expires_at = clock_timestamp() + make_interval(secs => $6::double precision)
WHERE scope_kind = $1 AND scope_key = $2 AND slot_index = $3
  AND holder_id = $4 AND generation = $5
  AND lease_expires_at > clock_timestamp()
RETURNING lease_expires_at";

const RELEASE_SLOT: &str = "
DELETE FROM ai_concurrency_slots
WHERE scope_kind = $1 AND scope_key = $2 AND slot_index = $3
  AND holder_id = $4 AND generation = $5";

/// Deletes slot rows whose lease expired at least [`SWEEP_RETENTION`] ago, at
/// most [`SWEEP_BATCH`] rows per call. Runs periodically so scopes that stop
/// being used do not grow the table; it is never on the dispatch path.
pub async fn sweep_ai_concurrency_slots(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let deleted = sqlx::query(
        "WITH expired AS (
           SELECT scope_kind, scope_key, slot_index FROM ai_concurrency_slots
           WHERE lease_expires_at < clock_timestamp() - make_interval(secs => $1::double precision)
           ORDER BY lease_expires_at
           LIMIT $2 FOR UPDATE SKIP LOCKED
         )
         DELETE FROM ai_concurrency_slots s USING expired e
         WHERE (s.scope_kind, s.scope_key, s.slot_index) =
               (e.scope_kind, e.scope_key, e.slot_index)",
    )
    .bind(SWEEP_RETENTION.as_secs_f64())
    .bind(SWEEP_BATCH)
    .execute(pool)
    .await?;
    Ok(deleted.rows_affected())
}

/// Leased-slot admission over PostgreSQL. One instance is one owner identity
/// (`holder_id`); its leases are only usable by that instance, exactly like a
/// job lease belongs to the worker that claimed it.
#[derive(Clone)]
pub struct PostgresAiConcurrencyAdmission {
    pool: PgPool,
    holder_id: Uuid,
    lease: Duration,
    metrics: Arc<AiConcurrencyMetrics>,
}

impl PostgresAiConcurrencyAdmission {
    /// The production shape: one instance per process, with the documented
    /// lease duration and a fresh owner identity. Metrics are shared process
    /// wide.
    pub fn new(pool: &PgPool) -> Self {
        Self::with_metrics(pool, deepref_ai::admission_metrics())
    }

    /// Shared telemetry, so every adapter in one process records into the same
    /// counters.
    pub fn with_metrics(pool: &PgPool, metrics: Arc<AiConcurrencyMetrics>) -> Self {
        Self {
            pool: pool.clone(),
            holder_id: Uuid::new_v4(),
            lease: DEFAULT_LEASE,
            metrics,
        }
    }

    /// Test seam: a named owner with a shorter lease, so expiry and takeover
    /// can be observed without waiting out the production TTL.
    pub fn with_holder_and_lease(
        pool: &PgPool,
        holder_id: Uuid,
        lease: Duration,
        metrics: Arc<AiConcurrencyMetrics>,
    ) -> Self {
        Self {
            pool: pool.clone(),
            holder_id,
            lease,
            metrics,
        }
    }

    pub const fn holder_id(&self) -> Uuid {
        self.holder_id
    }

    pub fn metrics(&self) -> Arc<AiConcurrencyMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Claims one slot of one scope key, or reports saturation. A claim that
    /// lost the race for its picked slot re-reads before giving up.
    async fn claim_slot(
        &self,
        tx: &mut Transaction<'_, sqlx::Postgres>,
        scope_kind: AiConcurrencyScopeKind,
        scope_key: &str,
        limit: u32,
    ) -> Result<Result<(SlotGrant, bool), ()>, AdmissionError> {
        for _ in 0..MAX_CLAIM_ATTEMPTS {
            let claimed = sqlx::query(CLAIM_SLOT)
                .bind(scope_kind.as_str())
                .bind(scope_key)
                .bind(limit as i32)
                .bind(self.holder_id)
                .bind(self.lease.as_secs_f64())
                .fetch_optional(&mut **tx)
                .await;
            let claimed = match claimed {
                Ok(row) => row,
                Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
                    self.metrics.record_saturation(scope_kind);
                    return Ok(Err(()));
                }
                Err(error) => return Err(persistence(error)),
            };
            if let Some(row) = claimed {
                let grant = SlotGrant {
                    scope_kind,
                    scope_key: scope_key.to_owned(),
                    slot_index: row.get::<i32, _>("slot_index") as u32,
                    holder_id: self.holder_id,
                    generation: row.get("generation"),
                    lease_expires_at: row.get("lease_expires_at"),
                };
                let reclaimed = !row.get::<bool, _>("inserted");
                return Ok(Ok((grant, reclaimed)));
            }
            let live: i64 = sqlx::query_scalar(LIVE_SLOTS)
                .bind(scope_kind.as_str())
                .bind(scope_key)
                .bind(limit as i32)
                .fetch_one(&mut **tx)
                .await
                .map_err(persistence)?;
            if live >= i64::from(limit) {
                self.metrics.record_saturation(scope_kind);
                return Ok(Err(()));
            }
            // Every slot below the limit is either claimed by another caller
            // in flight or was taken between our read and write. Try again:
            // the next attempt sees the committed state.
        }
        // Contention outlived the attempts. Refuse rather than wait: acquire
        // is non-blocking and a conservative refusal is a correct answer.
        self.metrics.record_saturation(scope_kind);
        Ok(Err(()))
    }

    /// Renews or frees one slot of one scope key. `0` affected rows means the
    /// lease no longer owns that slot.
    async fn touch_slot(
        &self,
        tx: &mut Transaction<'_, sqlx::Postgres>,
        grant: &SlotGrant,
        renew: bool,
    ) -> Result<u64, AdmissionError> {
        let slot_index = grant.slot_index as i32;
        if renew {
            let renewed = sqlx::query(RENEW_SLOT)
                .bind(grant.scope_kind.as_str())
                .bind(&grant.scope_key)
                .bind(slot_index)
                .bind(self.holder_id)
                .bind(grant.generation)
                .bind(self.lease.as_secs_f64())
                .execute(&mut **tx)
                .await
                .map_err(persistence)?;
            Ok(renewed.rows_affected())
        } else {
            let released = sqlx::query(RELEASE_SLOT)
                .bind(grant.scope_kind.as_str())
                .bind(&grant.scope_key)
                .bind(slot_index)
                .bind(self.holder_id)
                .bind(grant.generation)
                .execute(&mut **tx)
                .await
                .map_err(persistence)?;
            Ok(released.rows_affected())
        }
    }

    /// Both slots of a lease, in the fixed order. All-or-nothing: the caller
    /// rolls back unless both writes landed.
    async fn touch_lease(&self, lease: &SlotLease, renew: bool) -> Result<(), AdmissionError> {
        let mut tx = self.pool.begin().await.map_err(persistence)?;
        let touched = self.touch_slot(&mut tx, &lease.workspace, renew).await?
            + self.touch_slot(&mut tx, &lease.route, renew).await?;
        if touched != 2 {
            // One slot was taken over (or swept): the whole write is a no-op
            // and the holder is told it lost ownership.
            tx.rollback().await.map_err(persistence)?;
            self.metrics.record_lost_lease();
            return Err(AdmissionError::LeaseLost);
        }
        tx.commit().await.map_err(persistence)?;
        Ok(())
    }
}

impl AiConcurrencyAdmission for PostgresAiConcurrencyAdmission {
    fn acquire<'a>(&'a self, scope: AiConcurrencyScope) -> AdmissionFuture<'a, SlotLease> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let result = async {
                let keys = scope.keys();
                let mut tx = self.pool.begin().await.map_err(persistence)?;
                // A conflicting uncommitted claim must not become a capacity wait
                // (in particular for nested calls). Refuse conservatively on lock
                // contention, rather than waiting for the other provider call.
                sqlx::query("SET LOCAL lock_timeout = '10ms'")
                    .execute(&mut *tx)
                    .await
                    .map_err(persistence)?;
                // Fixed order: workspace first, then route. Both in one
                // transaction, so a refusal never leaves a slot held.
                let (workspace, workspace_reclaimed) = match self
                    .claim_slot(&mut tx, keys[0].0, &keys[0].1, keys[0].2)
                    .await?
                {
                    Ok(grant) => grant,
                    Err(()) => return Err(saturated(keys[0].0)),
                };
                let (route, route_reclaimed) = match self
                    .claim_slot(&mut tx, keys[1].0, &keys[1].1, keys[1].2)
                    .await?
                {
                    Ok(grant) => grant,
                    Err(()) => return Err(saturated(keys[1].0)),
                };
                let lease = SlotLease {
                    scope,
                    workspace,
                    route,
                };
                let touched = self.touch_slot(&mut tx, &lease.workspace, true).await?
                    + self.touch_slot(&mut tx, &lease.route, true).await?;
                if touched != 2 {
                    return Err(AdmissionError::LeaseLost);
                }
                tx.commit().await.map_err(persistence)?;
                self.metrics
                    .record_grant(AiConcurrencyScopeKind::Workspace, workspace_reclaimed);
                self.metrics
                    .record_grant(AiConcurrencyScopeKind::Route, route_reclaimed);
                Ok(lease)
            }
            .await;
            self.metrics.record_acquire(started.elapsed());
            result
        })
    }

    fn renew<'a>(&'a self, lease: &'a SlotLease) -> AdmissionFuture<'a, ()> {
        Box::pin(async move { self.touch_lease(lease, true).await })
    }

    fn release<'a>(&'a self, lease: SlotLease) -> AdmissionFuture<'a, ()> {
        Box::pin(async move { self.touch_lease(&lease, false).await })
    }

    /// A third of this adapter's lease, so one missed renewal never consumes
    /// the whole lease (the 05a/04a ratio).
    fn renew_interval(&self) -> Duration {
        (self.lease / 3).max(Duration::from_millis(1))
    }
}

fn saturated(scope_kind: AiConcurrencyScopeKind) -> AdmissionError {
    AdmissionError::Saturated { scope_kind }
}

fn persistence(error: sqlx::Error) -> AdmissionError {
    AdmissionError::Persistence(error.to_string())
}
