//! PostgreSQL owns the shared ceiling even while only some replicas can reach
//! Valkey. An accelerator may refuse early, but can never grant extra capacity.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use deepref_application::rate_limits::{
    NoopRateLimitObserver, RateLimitBackend, RateLimitDecision, RateLimitFuture, RateLimitObserver,
    RateLimitPolicy, RateLimitSubject, RateLimitUnavailable, RateLimiter,
};
use sqlx::PgPool;

const CONSUME: &str = "
INSERT INTO operational_rate_limits AS r
  (subject_kind, subject_id, api_key_id, workflow_id, window_started_at, used, ceiling, last_allowed)
VALUES ($1, $2, $3, $4, statement_timestamp(), 1, $5::bigint::integer, true)
ON CONFLICT (subject_kind, subject_id) DO UPDATE SET
  window_started_at = CASE WHEN r.window_started_at <= statement_timestamp() - interval '60 seconds'
    THEN statement_timestamp() ELSE r.window_started_at END,
  used = CASE WHEN r.window_started_at <= statement_timestamp() - interval '60 seconds'
    THEN 1 WHEN r.used < least(r.ceiling, EXCLUDED.ceiling) THEN r.used + 1 ELSE r.used END,
  ceiling = CASE WHEN r.window_started_at <= statement_timestamp() - interval '60 seconds'
    THEN EXCLUDED.ceiling ELSE least(r.ceiling, EXCLUDED.ceiling) END
, last_allowed = r.window_started_at <= statement_timestamp() - interval '60 seconds'
   OR r.used < least(r.ceiling, EXCLUDED.ceiling)
RETURNING last_allowed";

#[derive(Clone)]
pub struct PostgresRateLimiter {
    pool: PgPool,
    accelerator: Option<Arc<dyn RateLimiter>>,
    observer: Arc<dyn RateLimitObserver>,
}

impl PostgresRateLimiter {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            accelerator: None,
            observer: Arc::new(NoopRateLimitObserver),
        }
    }

    /// The accelerator is advisory: errors fall back to the SAME PostgreSQL
    /// counter that healthy replicas consume. No independent fallback budget.
    pub fn with_accelerator(mut self, accelerator: Arc<dyn RateLimiter>) -> Self {
        self.accelerator = Some(accelerator);
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn RateLimitObserver>) -> Self {
        self.observer = observer;
        self
    }

    async fn consume(
        &self,
        subject: RateLimitSubject,
        policy: RateLimitPolicy,
    ) -> Result<RateLimitDecision, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL lock_timeout = '250ms'")
            .execute(&mut *tx)
            .await?;
        let (api_key, workflow) = match subject {
            RateLimitSubject::ApiKey(_) => (Some(subject.id()), None),
            RateLimitSubject::WorkflowWebhook(_) | RateLimitSubject::WorkflowEmail(_) => {
                (None, Some(subject.id()))
            }
        };
        let allowed: bool = sqlx::query_scalar(CONSUME)
            .bind(subject.kind())
            .bind(subject.id())
            .bind(api_key)
            .bind(workflow)
            .bind(i64::from(policy.limit()))
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(if allowed {
            RateLimitDecision::Allowed
        } else {
            RateLimitDecision::Limited
        })
    }
}

impl RateLimiter for PostgresRateLimiter {
    fn check(&self, subject: RateLimitSubject, policy: RateLimitPolicy) -> RateLimitFuture<'_> {
        Box::pin(async move {
            if let Some(accelerator) = &self.accelerator {
                let started = Instant::now();
                let outcome = accelerator.check(subject, policy).await;
                self.observer.decision(
                    subject.purpose(),
                    RateLimitBackend::Valkey,
                    outcome,
                    started.elapsed(),
                );
                match outcome {
                    Ok(RateLimitDecision::Limited) => return Ok(RateLimitDecision::Limited),
                    Ok(RateLimitDecision::Allowed) => {}
                    Err(_) => self.observer.fallback(subject.purpose()),
                }
            }
            let started = Instant::now();
            let outcome =
                tokio::time::timeout(Duration::from_secs(2), self.consume(subject, policy))
                    .await
                    .map_err(|_| RateLimitUnavailable)
                    .and_then(|result| result.map_err(|_| RateLimitUnavailable));
            self.observer.decision(
                subject.purpose(),
                RateLimitBackend::Postgres,
                outcome,
                started.elapsed(),
            );
            outcome
        })
    }
}
