#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Real shared counter behavior, including partial acceleration failure.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use deepref_app::rate_limits::PostgresRateLimiter;
use deepref_application::rate_limits::{
    RateLimitBackend, RateLimitDecision as Decision, RateLimitFuture, RateLimitObserver,
    RateLimitPolicy, RateLimitPurpose, RateLimitSubject, RateLimitUnavailable, RateLimiter,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::task::JoinSet;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

async fn subject(pool: &PgPool) -> RateLimitSubject {
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'rate test')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    let workflow: Uuid = sqlx::query_scalar("INSERT INTO workflows(project_id,name,webhook_token,webhook_secret,email_token,created_by_kind,created_by_id) VALUES($1,'limits',$2,'secret',$3,'system','limits-test') RETURNING id")
        .bind(project).bind(Uuid::new_v4().to_string()).bind(Uuid::new_v4().to_string()).fetch_one(pool).await.unwrap();
    RateLimitSubject::WorkflowWebhook(workflow)
}

fn policy(limit: u32) -> RateLimitPolicy {
    RateLimitPolicy::per_minute(limit).unwrap()
}

struct Unavailable;
impl RateLimiter for Unavailable {
    fn check(&self, _: RateLimitSubject, _: RateLimitPolicy) -> RateLimitFuture<'_> {
        Box::pin(async { Err(RateLimitUnavailable) })
    }
}

#[derive(Default)]
struct Observations {
    fallbacks: AtomicUsize,
    pg_allowed: AtomicUsize,
    pg_limited: AtomicUsize,
    valkey_errors: AtomicUsize,
}

impl RateLimitObserver for Observations {
    fn decision(
        &self,
        purpose: RateLimitPurpose,
        backend: RateLimitBackend,
        outcome: Result<Decision, RateLimitUnavailable>,
        _: Duration,
    ) {
        assert_eq!(purpose, RateLimitPurpose::WorkflowWebhook);
        match (backend, outcome) {
            (RateLimitBackend::Postgres, Ok(Decision::Allowed)) => {
                self.pg_allowed.fetch_add(1, Ordering::Relaxed);
            }
            (RateLimitBackend::Postgres, Ok(Decision::Limited)) => {
                self.pg_limited.fetch_add(1, Ordering::Relaxed);
            }
            (RateLimitBackend::Valkey, Err(_)) => {
                self.valkey_errors.fetch_add(1, Ordering::Relaxed);
            }
            _ => panic!("unexpected observation"),
        }
    }
    fn fallback(&self, purpose: RateLimitPurpose) {
        assert_eq!(purpose, RateLimitPurpose::WorkflowWebhook);
        self.fallbacks.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn two_pools_and_partitioned_acceleration_share_one_ceiling() {
    let Some(pool) = database().await else {
        return;
    };
    let other = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    let subject = subject(&pool).await;
    let observations = Arc::new(Observations::default());
    let healthy = PostgresRateLimiter::new(pool.clone()).with_observer(observations.clone());
    let partitioned = PostgresRateLimiter::new(other.clone())
        .with_accelerator(Arc::new(Unavailable))
        .with_observer(observations.clone());
    let mut calls = JoinSet::new();
    for i in 0..32 {
        let limiter = if i % 2 == 0 {
            healthy.clone()
        } else {
            partitioned.clone()
        };
        calls.spawn(async move { limiter.check(subject, policy(7)).await.unwrap() });
    }
    let mut allowed = 0;
    while let Some(result) = calls.join_next().await {
        if result.unwrap() == Decision::Allowed {
            allowed += 1;
        }
    }
    assert_eq!(allowed, 7);
    let used: i32 = sqlx::query_scalar("SELECT used FROM operational_rate_limits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(used, 7, "refusals never grow the counter");
    assert_eq!(observations.pg_allowed.load(Ordering::Relaxed), 7);
    assert_eq!(observations.pg_limited.load(Ordering::Relaxed), 25);
    assert_eq!(observations.valkey_errors.load(Ordering::Relaxed), 16);
    assert_eq!(observations.fallbacks.load(Ordering::Relaxed), 16);
    other.close().await;
}

#[tokio::test]
async fn expiry_policy_changes_identity_separation_and_cascade_are_real() {
    let Some(pool) = database().await else {
        return;
    };
    let subject = subject(&pool).await;
    let limiter = PostgresRateLimiter::new(pool.clone());
    assert_eq!(
        limiter.check(subject, policy(5)).await.unwrap(),
        Decision::Allowed
    );
    assert_eq!(
        limiter.check(subject, policy(1)).await.unwrap(),
        Decision::Limited
    );
    assert_eq!(
        limiter.check(subject, policy(5)).await.unwrap(),
        Decision::Limited,
        "lower ceiling persists even after denial"
    );
    assert_eq!(
        limiter
            .check(RateLimitSubject::WorkflowEmail(subject.id()), policy(1))
            .await
            .unwrap(),
        Decision::Allowed
    );
    sqlx::query("UPDATE operational_rate_limits SET window_started_at = statement_timestamp() - interval '61 seconds'").execute(&pool).await.unwrap();
    assert_eq!(
        limiter.check(subject, policy(2)).await.unwrap(),
        Decision::Allowed
    );
    assert_eq!(
        limiter.check(subject, policy(2)).await.unwrap(),
        Decision::Allowed
    );
    assert_eq!(
        limiter.check(subject, policy(2)).await.unwrap(),
        Decision::Limited
    );
    sqlx::query("DELETE FROM workflows WHERE id=$1")
        .bind(subject.id())
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM operational_rate_limits")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        limiter.check(subject, policy(1)).await,
        Err(RateLimitUnavailable),
        "unknown identity cannot allocate a counter"
    );
}

#[tokio::test]
async fn held_counter_lock_fails_closed_without_losing_prior_budget() {
    let Some(pool) = database().await else {
        return;
    };
    let subject = subject(&pool).await;
    let limiter = PostgresRateLimiter::new(pool.clone());
    assert_eq!(
        limiter.check(subject, policy(2)).await.unwrap(),
        Decision::Allowed
    );
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM operational_rate_limits FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), limiter.check(subject, policy(2)))
        .await
        .expect("bounded lock wait");
    assert_eq!(result, Err(RateLimitUnavailable));
    blocker.rollback().await.unwrap();
    assert_eq!(
        limiter.check(subject, policy(2)).await.unwrap(),
        Decision::Allowed
    );
    assert_eq!(
        limiter.check(subject, policy(2)).await.unwrap(),
        Decision::Limited
    );
}
