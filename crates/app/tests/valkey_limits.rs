#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! A real Valkey and PostgreSQL, required by the strict CI fixture runner.

use std::{sync::Arc, time::Duration};

use deepref_app::rate_limits::PostgresRateLimiter;
use deepref_application::rate_limits::{
    RateLimitDecision as Decision, RateLimitPolicy, RateLimitSubject, RateLimiter,
};
use deepref_integrations::valkey_fixture::ValkeyFixtureControl;
use tokio::task::JoinSet;
use uuid::Uuid;

#[tokio::test]
async fn real_atomic_ttl_reset_partition_and_ambiguous_reply_keep_the_pg_ceiling() {
    let Some(address) = std::env::var("TEST_VALKEY_URL").ok() else {
        assert_ne!(
            std::env::var("DEEPREF_REQUIRE_VALKEY").as_deref(),
            Ok("1"),
            "strict Valkey fixture URL is missing"
        );
        return;
    };
    let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8)
        .await
        .expect("required PostgreSQL fixture");
    let namespace = format!("contract-{}", Uuid::new_v4());
    let mut control =
        ValkeyFixtureControl::connect(&address, &namespace, Duration::from_millis(100))
            .await
            .unwrap();
    let accelerator = Arc::new(control.accelerator().unwrap());
    let subject = RateLimitSubject::WorkflowWebhook(Uuid::new_v4());
    let policy = RateLimitPolicy::per_minute(7).unwrap();
    let mut checks = JoinSet::new();
    for _ in 0..32 {
        let limiter = Arc::clone(&accelerator);
        checks.spawn(async move { limiter.check(subject, policy).await.unwrap() });
    }
    let mut count = 0;
    while let Some(check) = checks.join_next().await {
        if check.unwrap() == Decision::Allowed {
            count += 1;
        }
    }
    assert_eq!(count, 7, "Lua is atomic across concurrent requests");
    let ttl = control.ttl(subject).await.unwrap();
    assert!((1..=60_000).contains(&ttl));
    control.expire_now(subject).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        accelerator.check(subject, policy).await.unwrap(),
        Decision::Allowed,
        "server expiry frees only the accelerator"
    );

    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'Valkey contract')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let workflow: Uuid = sqlx::query_scalar("INSERT INTO workflows(project_id,name,webhook_token,webhook_secret,email_token,created_by_kind,created_by_id) VALUES($1,'limits',$2,'secret',$3,'system','limits-test') RETURNING id")
        .bind(project).bind(Uuid::new_v4().to_string()).bind(Uuid::new_v4().to_string()).fetch_one(&pool).await.unwrap();
    let subject = RateLimitSubject::WorkflowWebhook(workflow);
    let healthy = PostgresRateLimiter::new(pool.clone()).with_accelerator(accelerator.clone());
    let fallback = PostgresRateLimiter::new(pool.clone());
    let policy = RateLimitPolicy::per_minute(3).unwrap();
    assert_eq!(
        healthy.check(subject, policy).await.unwrap(),
        Decision::Allowed
    );
    assert_eq!(
        fallback.check(subject, policy).await.unwrap(),
        Decision::Allowed
    );
    control.clear(subject).await.unwrap();
    assert_eq!(
        healthy.check(subject, policy).await.unwrap(),
        Decision::Allowed
    );
    assert_eq!(
        healthy.check(subject, policy).await.unwrap(),
        Decision::Limited,
        "Valkey reset cannot reset the shared PG budget"
    );
    assert_eq!(
        fallback.check(subject, policy).await.unwrap(),
        Decision::Limited
    );

    // Redis queues the mutating script beyond the client deadline. Its eventual
    // execution is deliberately ambiguous to that caller; PG still caps grants.
    let policy = RateLimitPolicy::per_minute(9).unwrap();
    sqlx::query("UPDATE operational_rate_limits SET window_started_at=statement_timestamp()-interval '61 seconds'").execute(&pool).await.unwrap();
    control.clear(subject).await.unwrap();
    control.pause_writes().await.unwrap();
    assert_eq!(
        healthy.check(subject, policy).await.unwrap(),
        Decision::Allowed,
        "timeout falls back through canonical PG"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let used = control.used(subject).await.unwrap();
    assert_eq!(
        used, 1,
        "timed-out script really executed; no blind mutation retry"
    );
    for _ in 1..9 {
        assert_eq!(
            fallback.check(subject, policy).await.unwrap(),
            Decision::Allowed
        );
    }
    assert_eq!(
        healthy.check(subject, policy).await.unwrap(),
        Decision::Limited
    );
    control.clear(subject).await.unwrap();

    // A client constructed during an outage must recover without a process
    // restart. The loopback proxy starts only after the first refusal.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    drop(listener);
    let lazy = ValkeyFixtureControl::lazy_accelerator(
        format!("redis://{proxy_address}"),
        format!("startup-{}", Uuid::new_v4()),
        Duration::from_millis(200),
    )
    .unwrap();
    assert!(
        lazy.check(subject, policy).await.is_err(),
        "unavailable startup endpoint refuses within its deadline"
    );
    let listener = tokio::net::TcpListener::bind(proxy_address).await.unwrap();
    let backend = control.endpoint().to_owned();
    let proxy = tokio::spawn(async move {
        let (mut downstream, _) = listener.accept().await.unwrap();
        let mut upstream = tokio::net::TcpStream::connect(backend).await.unwrap();
        tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await
    });
    assert_eq!(
        lazy.check(subject, policy).await.unwrap(),
        Decision::Allowed
    );
    proxy.abort();
    let _ = proxy.await;
}
