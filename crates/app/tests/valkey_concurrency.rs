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

struct DelayedHold {
    real: Arc<dyn ConcurrencyMirror>,
    control: tokio::sync::Mutex<ValkeyFixtureControl>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ConcurrencyMirror for DelayedHold {
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
            // Pause after both CHECKs and PG acquisition, immediately before
            // the first production HOLD. The server executes that mutation
            // after the real client deadline has already elapsed.
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                self.control.lock().await.pause_writes().await.unwrap();
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
    fn release(&self, _: MirrorScope, _: MirrorLeaseId) -> MirrorFuture<'_, ()> {
        // A partition during release must not leave canonical PG capacity held.
        // No attempt to replay an ambiguous mutation is made by this shim.
        Box::pin(async { Err(deepref_application::concurrency_mirror::MirrorUnavailable) })
    }
}

#[tokio::test]
async fn delayed_real_zset_hold_is_only_a_restrictive_ghost_after_pg_release_and_expires() {
    let Some((pool, control)) = fixture().await else {
        return;
    };
    let real = Arc::new(control.concurrency_mirror().unwrap());
    // Preload HOLD and RELEASE before pausing server writes. Otherwise a cold
    // EVALSHA's NOSCRIPT -> SCRIPT LOAD can time out before HOLD is ever sent.
    let warm_scope = MirrorScope::Workspace(WorkspaceId::new(Uuid::new_v4()));
    let warm_lease = MirrorLeaseId {
        holder: Uuid::new_v4(),
        generation: 1,
        slot: 0,
    };
    real.hold(
        warm_scope,
        warm_lease,
        MirrorPolicy::new(1, Duration::from_secs(1)).unwrap(),
    )
    .await
    .unwrap();
    real.release(warm_scope, warm_lease).await.unwrap();
    let mirror = Arc::new(DelayedHold {
        real,
        control: tokio::sync::Mutex::new(control),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let primary = canonical(&pool, Duration::from_secs(2));
    let observed = Arc::new(HoldTimeoutCount::default());
    let admission = MirroredAiConcurrencyAdmission::new(primary.clone(), mirror.clone())
        .with_observer(observed.clone());
    let scope = scope();
    let held = admission.acquire(scope.clone()).await.unwrap();
    assert!(
        observed.0.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "a real HOLD reply timed out"
    );
    assert_eq!(
        mirror.calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one outer HOLD per scope; no blind application replay"
    );
    assert!(matches!(
        primary.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    admission.release(held).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_concurrency_slots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "both canonical slots released despite optional cache partition"
    );
    let key = MirrorScope::Workspace(scope.workspace_id);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if mirror
                .control
                .lock()
                .await
                .concurrency_count(key)
                .await
                .unwrap()
                > 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed-out mutation eventually executed on real server");
    // A ghost may refuse useful work; it cannot grant past the PG ceiling.
    assert!(matches!(
        admission.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    let pg_only = primary.acquire(scope.clone()).await.unwrap();
    primary.release(pg_only).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if mirror
                .control
                .lock()
                .await
                .concurrency_count(key)
                .await
                .unwrap()
                == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("restrictive ghost has bounded real TTL");
    let recovered = admission.acquire(scope).await.unwrap();
    admission.release(recovered).await.unwrap();
}

#[derive(Default)]
struct HoldTimeoutCount(std::sync::atomic::AtomicUsize);
impl deepref_application::concurrency_mirror::ConcurrencyMirrorObserver for HoldTimeoutCount {
    fn observe(
        &self,
        _: deepref_application::concurrency_mirror::MirrorScopeKind,
        operation: deepref_application::concurrency_mirror::MirrorOperation,
        outcome: deepref_application::concurrency_mirror::MirrorOutcome,
        _: Duration,
    ) {
        if matches!(
            operation,
            deepref_application::concurrency_mirror::MirrorOperation::Hold
        ) && matches!(
            outcome,
            deepref_application::concurrency_mirror::MirrorOutcome::Unavailable
        ) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// Real Valkey mirror that can be switched off between calls, modelling an
/// outage that starts while leases are already held.
struct Switchable {
    real: Arc<dyn ConcurrencyMirror>,
    down: Arc<std::sync::atomic::AtomicBool>,
}
impl Switchable {
    fn down(&self) -> bool {
        self.down.load(std::sync::atomic::Ordering::SeqCst)
    }
}
impl ConcurrencyMirror for Switchable {
    fn check(&self, scope: MirrorScope, policy: MirrorPolicy) -> MirrorFuture<'_, MirrorDecision> {
        if self.down() {
            return Box::pin(async {
                Err(deepref_application::concurrency_mirror::MirrorUnavailable)
            });
        }
        self.real.check(scope, policy)
    }
    fn hold(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, MirrorDecision> {
        if self.down() {
            return Box::pin(async {
                Err(deepref_application::concurrency_mirror::MirrorUnavailable)
            });
        }
        self.real.hold(scope, lease, policy)
    }
    fn refresh(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, ()> {
        if self.down() {
            return Box::pin(async {
                Err(deepref_application::concurrency_mirror::MirrorUnavailable)
            });
        }
        self.real.refresh(scope, lease, policy)
    }
    fn release(&self, scope: MirrorScope, lease: MirrorLeaseId) -> MirrorFuture<'_, ()> {
        if self.down() {
            return Box::pin(async {
                Err(deepref_application::concurrency_mirror::MirrorUnavailable)
            });
        }
        self.real.release(scope, lease)
    }
}

/// Live PG leases on the workspace key of `scope`.
async fn slot_count(pool: &sqlx::PgPool, scope: &AiConcurrencyScope) -> i64 {
    let [(_, workspace_key, _), _] = scope.keys();
    sqlx::query_scalar(
        "SELECT count(*) FROM ai_concurrency_slots
         WHERE scope_kind='workspace' AND scope_key=$1 AND lease_expires_at >= clock_timestamp()",
    )
    .bind(workspace_key)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn valkey_outage_mid_lease_keeps_the_pg_ceiling_across_two_instances() {
    let Some((pool, control)) = fixture().await else {
        return;
    };
    let down = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let real: Arc<dyn ConcurrencyMirror> = Arc::new(control.concurrency_mirror().unwrap());
    // Two API instances: separate PG holders, one shared Valkey namespace.
    let instance = |pool: &sqlx::PgPool| {
        MirroredAiConcurrencyAdmission::new(
            canonical(pool, Duration::from_secs(2)),
            Arc::new(Switchable {
                real: real.clone(),
                down: down.clone(),
            }),
        )
    };
    let first = instance(&pool);
    let second = instance(&pool);
    let scope = scope();
    let held = first.acquire(scope.clone()).await.unwrap();

    down.store(true, std::sync::atomic::Ordering::SeqCst);
    first.renew(&held).await.unwrap();
    assert!(
        matches!(
            second.acquire(scope.clone()).await,
            Err(AdmissionError::Saturated { .. })
        ),
        "an unavailable mirror must fall back to the PG ceiling, not admit"
    );
    first.release(held).await.unwrap();
    assert_eq!(slot_count(&pool, &scope).await, 0);

    // With Valkey still down, the PG slot freed by `release` is admitted and
    // the ceiling still holds for the other instance.
    let fallback = second.acquire(scope.clone()).await.unwrap();
    assert!(matches!(
        first.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    second.release(fallback).await.unwrap();
    assert_eq!(slot_count(&pool, &scope).await, 0);
}

#[tokio::test]
async fn an_expired_pg_lease_is_reclaimed_and_its_late_release_cannot_free_the_new_holder() {
    let Some((pool, control)) = fixture().await else {
        return;
    };
    let mirror: Arc<dyn ConcurrencyMirror> = Arc::new(control.concurrency_mirror().unwrap());
    let stale = MirroredAiConcurrencyAdmission::new(
        canonical(&pool, Duration::from_millis(300)),
        mirror.clone(),
    );
    let fresh =
        MirroredAiConcurrencyAdmission::new(canonical(&pool, Duration::from_secs(2)), mirror);
    let scope = scope();
    let expired = stale.acquire(scope.clone()).await.unwrap();
    assert!(matches!(
        fresh.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));

    // The stale holder stops heartbeating; both the PG lease and its mirror
    // entry (bounded by the same renew interval) expire.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let reclaimed = fresh.acquire(scope.clone()).await.unwrap();
    assert_eq!(
        stale.renew(&expired).await.unwrap_err(),
        AdmissionError::LeaseLost
    );
    let _ = stale.release(expired).await;
    assert_eq!(
        slot_count(&pool, &scope).await,
        1,
        "a late release from the expired holder cannot free the new lease"
    );
    assert!(matches!(
        stale.acquire(scope.clone()).await,
        Err(AdmissionError::Saturated { .. })
    ));
    fresh.renew(&reclaimed).await.unwrap();
    fresh.release(reclaimed).await.unwrap();
    assert_eq!(slot_count(&pool, &scope).await, 0);
}
