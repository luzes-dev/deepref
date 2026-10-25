#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use deepref_ai::{
    AdmissionError, AiConcurrencyAdmission, AiConcurrencyLimits, AiConcurrencyMetrics,
    AiConcurrencyScope,
};
use deepref_app::{
    PostgresAiConcurrencyAdmission, mirrored_concurrency::MirroredAiConcurrencyAdmission,
};
use deepref_application::concurrency_mirror::{
    ConcurrencyMirror, MirrorDecision, MirrorFuture, MirrorLeaseId, MirrorPolicy, MirrorScope,
};
use deepref_domain::WorkspaceId;
use deepref_integrations::valkey_fixture::ValkeyFixtureControl;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

async fn fixture() -> Option<(sqlx::PgPool, ValkeyFixtureControl)> {
    let Ok(address) = std::env::var("TEST_VALKEY_URL") else {
        assert_ne!(
            std::env::var("DEEPREF_REQUIRE_VALKEY").as_deref(),
            Ok("1"),
            "required Valkey fixture missing"
        );
        return None;
    };
    let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8)
        .await
        .expect("disposable PostgreSQL");
    let control = ValkeyFixtureControl::connect(
        &address,
        &format!("contract-{}", Uuid::new_v4()),
        Duration::from_millis(100),
    )
    .await
    .unwrap();
    Some((pool, control))
}

fn scope() -> AiConcurrencyScope {
    AiConcurrencyScope::new(
        WorkspaceId::new(Uuid::new_v4()),
        "provider",
        Uuid::new_v4().to_string(),
        AiConcurrencyLimits {
            workspace: 1,
            route: 1,
        },
    )
}

fn canonical(pool: &sqlx::PgPool, ttl: Duration) -> Arc<PostgresAiConcurrencyAdmission> {
    Arc::new(PostgresAiConcurrencyAdmission::with_holder_and_lease(
        pool,
        Uuid::new_v4(),
        ttl,
        Arc::new(AiConcurrencyMetrics::default()),
    ))
}

#[tokio::test]
async fn real_mirror_is_atomic_expires_and_old_generations_cannot_release_new_members() {
    let Some((_pool, mut control)) = fixture().await else {
        return;
    };
    let mirror = Arc::new(control.concurrency_mirror().unwrap());
    let key = MirrorScope::Workspace(WorkspaceId::new(Uuid::new_v4()));
    let policy = MirrorPolicy::new(2, Duration::from_secs(1)).unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let mirror = mirror.clone();
        tasks.spawn(async move {
            mirror
                .hold(
                    key,
                    MirrorLeaseId {
                        holder: Uuid::new_v4(),
                        generation: 1,
                        slot: 0,
                    },
                    policy,
                )
                .await
                .unwrap()
        });
    }
    let mut granted = 0;
    while let Some(result) = tasks.join_next().await {
        granted += usize::from(result.unwrap() == MirrorDecision::Available);
    }
    assert_eq!(granted, 2);
    assert_eq!(control.concurrency_count(key).await.unwrap(), 2);
    control.clear_concurrency(key).await.unwrap();
    let old = MirrorLeaseId {
        holder: Uuid::new_v4(),
        generation: 1,
        slot: 0,
    };
    let new = MirrorLeaseId {
        generation: 2,
        ..old
    };
    let short = MirrorPolicy::new(1, Duration::from_millis(30)).unwrap();
    assert_eq!(
        mirror.hold(key, old, short).await.unwrap(),
        MirrorDecision::Available
    );
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        mirror.hold(key, new, policy).await.unwrap(),
        MirrorDecision::Available
    );
    mirror.release(key, old).await.unwrap();
    assert_eq!(control.concurrency_count(key).await.unwrap(), 1);
    mirror.release(key, new).await.unwrap();
    mirror.refresh(key, new, policy).await.unwrap();
    assert_eq!(
        control.concurrency_count(key).await.unwrap(),
        0,
        "late refresh cannot resurrect release"
    );
}

#[tokio::test]
async fn reset_partial_partition_and_redis_loss_never_add_to_the_shared_pg_ceiling() {
    let Some((pool, mut control)) = fixture().await else {
        return;
    };
    let database = canonical(&pool, Duration::from_secs(2));
    let scope = scope();
    let healthy = MirroredAiConcurrencyAdmission::new(
        database.clone(),
        Arc::new(control.concurrency_mirror().unwrap()),
    );
    let other = canonical(&pool, Duration::from_secs(2));
    let held = healthy.acquire(scope.clone()).await.unwrap();
    control
        .clear_concurrency(MirrorScope::Workspace(scope.workspace_id))
        .await
        .unwrap();
    control
        .clear_concurrency(MirrorScope::Route(
            Sha256::digest(scope.route_key().as_bytes()).into(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        other.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    assert!(matches!(
        healthy.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    healthy.renew(&held).await.unwrap();
    assert_eq!(
        control
            .concurrency_count(MirrorScope::Workspace(scope.workspace_id))
            .await
            .unwrap(),
        0,
        "cache reset is an absent mirror, not lost database ownership"
    );
    healthy.release(held).await.unwrap();
    let fallback_lease = other.acquire(scope.clone()).await.unwrap();
    assert!(matches!(
        healthy.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    other.release(fallback_lease).await.unwrap();

    let bad_config =
        deepref_integrations::valkey_fixture::ValkeyFixtureControl::lazy_concurrency_mirror(
            "redis://127.0.0.1:1".to_owned(),
            format!("contract-{}", Uuid::new_v4()),
            Duration::from_millis(20),
        )
        .unwrap();
    let unavailable = MirroredAiConcurrencyAdmission::new(database, Arc::new(bad_config));
    let held = unavailable.acquire(scope.clone()).await.unwrap();
    assert!(matches!(
        other.acquire(scope).await,
        Err(AdmissionError::Saturated { .. })
    ));
    unavailable.renew(&held).await.unwrap();
    unavailable.release(held).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_concurrency_slots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

struct RaceMirror {
    real: Arc<dyn ConcurrencyMirror>,
}
impl ConcurrencyMirror for RaceMirror {
    fn check(&self, scope: MirrorScope, policy: MirrorPolicy) -> MirrorFuture<'_, MirrorDecision> {
        self.real.check(scope, policy)
    }
    fn hold(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, MirrorDecision> {
        Box::pin(async move {
            if matches!(scope, MirrorScope::Route(_)) {
                self.real
                    .hold(
                        scope,
                        MirrorLeaseId {
                            holder: Uuid::new_v4(),
                            generation: 1,
                            slot: 0,
                        },
                        policy,
                    )
                    .await?;
            }
            self.real.hold(scope, lease, policy).await
        })
    }
    fn refresh(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, ()> {
        self.real.refresh(scope, lease, policy)
    }
    fn release(&self, scope: MirrorScope, lease: MirrorLeaseId) -> MirrorFuture<'_, ()> {
        self.real.release(scope, lease)
    }
}

#[tokio::test]
async fn a_racing_mirror_refusal_refunds_both_pg_slots_before_reporting_saturation() {
    let Some((pool, mut control)) = fixture().await else {
        return;
    };
    let scope = scope();
    let mirror = RaceMirror {
        real: Arc::new(control.concurrency_mirror().unwrap()),
    };
    let admission = MirroredAiConcurrencyAdmission::new(
        canonical(&pool, Duration::from_secs(2)),
        Arc::new(mirror),
    );
    assert!(matches!(
        admission.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    assert_eq!(
        control
            .concurrency_count(MirrorScope::Workspace(scope.workspace_id))
            .await
            .unwrap(),
        0
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_concurrency_slots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[derive(Default)]
struct ObservationCount(std::sync::atomic::AtomicUsize);
impl deepref_application::concurrency_mirror::ConcurrencyMirrorObserver for ObservationCount {
    fn observe(
        &self,
        _: deepref_application::concurrency_mirror::MirrorScopeKind,
        operation: deepref_application::concurrency_mirror::MirrorOperation,
        outcome: deepref_application::concurrency_mirror::MirrorOutcome,
        _: Duration,
    ) {
        if matches!(
            operation,
            deepref_application::concurrency_mirror::MirrorOperation::Refresh
        ) && matches!(
            outcome,
            deepref_application::concurrency_mirror::MirrorOutcome::Unavailable
        ) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

struct StalledRefresh {
    real: Arc<dyn ConcurrencyMirror>,
}
impl ConcurrencyMirror for StalledRefresh {
    fn check(&self, scope: MirrorScope, policy: MirrorPolicy) -> MirrorFuture<'_, MirrorDecision> {
        self.real.check(scope, policy)
    }
    fn hold(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, MirrorDecision> {
        self.real.hold(scope, lease, policy)
    }
    fn refresh(&self, _: MirrorScope, _: MirrorLeaseId, _: MirrorPolicy) -> MirrorFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
    fn release(&self, scope: MirrorScope, lease: MirrorLeaseId) -> MirrorFuture<'_, ()> {
        self.real.release(scope, lease)
    }
}

#[tokio::test]
async fn a_stalled_optional_refresh_cannot_consume_the_pg_renewal_deadline_or_mask_lease_loss() {
    let Some((pool, control)) = fixture().await else {
        return;
    };
    let mirror = StalledRefresh {
        real: Arc::new(control.concurrency_mirror().unwrap()),
    };
    let observations = Arc::new(ObservationCount::default());
    let admission = MirroredAiConcurrencyAdmission::new(
        canonical(&pool, Duration::from_secs(1)),
        Arc::new(mirror),
    )
    .with_observer(observations.clone());
    let held = admission.acquire(scope()).await.unwrap();
    tokio::time::timeout(admission.renew_interval(), admission.renew(&held))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        observations.0.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "each bounded refresh timeout is observed without leaking identities"
    );
    sqlx::query("DELETE FROM ai_concurrency_slots WHERE holder_id=$1")
        .bind(held.workspace.holder_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        admission.renew(&held).await.unwrap_err(),
        AdmissionError::LeaseLost
    );
}

struct UnconfirmedRelease {
    real: Arc<dyn AiConcurrencyAdmission>,
    held: std::sync::Mutex<Option<deepref_ai::SlotLease>>,
}
impl AiConcurrencyAdmission for UnconfirmedRelease {
    fn acquire<'a>(
        &'a self,
        scope: AiConcurrencyScope,
    ) -> deepref_ai::AdmissionFuture<'a, deepref_ai::SlotLease> {
        Box::pin(async move {
            let lease = self.real.acquire(scope).await?;
            *self.held.lock().unwrap() = Some(lease.clone());
            Ok(lease)
        })
    }
    fn renew<'a>(
        &'a self,
        lease: &'a deepref_ai::SlotLease,
    ) -> deepref_ai::AdmissionFuture<'a, ()> {
        self.real.renew(lease)
    }
    fn release<'a>(&'a self, _: deepref_ai::SlotLease) -> deepref_ai::AdmissionFuture<'a, ()> {
        Box::pin(async { Err(AdmissionError::Persistence("release_unconfirmed".into())) })
    }
    fn renew_interval(&self) -> Duration {
        self.real.renew_interval()
    }
}

#[tokio::test]
async fn unconfirmed_pg_refund_is_never_misreported_as_clean_saturation() {
    let Some((pool, control)) = fixture().await else {
        return;
    };
    let real = canonical(&pool, Duration::from_secs(2));
    let faulty = Arc::new(UnconfirmedRelease {
        real: real.clone(),
        held: std::sync::Mutex::new(None),
    });
    let mirror = RaceMirror {
        real: Arc::new(control.concurrency_mirror().unwrap()),
    };
    let admission = MirroredAiConcurrencyAdmission::new(faulty.clone(), Arc::new(mirror));
    assert_eq!(
        admission.acquire(scope()).await.unwrap_err(),
        AdmissionError::Persistence("release_unconfirmed".into())
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_concurrency_slots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "unconfirmed refund keeps canonical slots until proven release or expiry"
    );
    let held = faulty.held.lock().unwrap().take().unwrap();
    real.release(held).await.unwrap();
}
