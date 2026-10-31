#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::panic_in_result_fn
)]

//! Real PostgreSQL concurrency tests for AI concurrency admission.
//!
//! Every test races real tasks and real transactions against the leased-slot
//! table: no mock clock and no in-process lock stands in for the database.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use deepref_ai::{
    AdmissionError, AdmissionFuture, AiConcurrencyAdmission, AiConcurrencyLimits,
    AiConcurrencyMetrics, AiConcurrencyScope, AiConcurrencyScopeKind, AiError, AiFuture, AiGateway,
    AiUsageContext, CompletionRequest, GatewayCompletion, MeteredGateway, ModelParameters,
    ModelProfile, PreparedCompletionRequest, PriceBook, ProviderDialect, ResolvedModel, SlotLease,
    UsagePrincipal, structured_request_body,
};
use deepref_app::{
    PostgresAiConcurrencyAdmission, PostgresUsageLedger, sweep_ai_concurrency_slots,
};
use deepref_domain::{ProjectId, WorkspaceId};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

const SHORT_LEASE: Duration = Duration::from_secs(3);

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env_first(
        &["TEST_DATABASE_URL", "DATABASE_URL"],
        24,
    )
    .await
}

/// A second connection pool to the same isolated database: two "API
/// instances" must share one limit even though they share no pool.
async fn second_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .connect(url)
        .await
        .expect("second instance connects to the same database")
}

fn adapter(pool: &PgPool, lease: Duration) -> PostgresAiConcurrencyAdmission {
    PostgresAiConcurrencyAdmission::with_holder_and_lease(
        pool,
        Uuid::new_v4(),
        lease,
        Arc::new(AiConcurrencyMetrics::default()),
    )
}

fn scope(
    workspace: WorkspaceId,
    provider: &str,
    model: &str,
    limits: AiConcurrencyLimits,
) -> AiConcurrencyScope {
    AiConcurrencyScope::new(workspace, provider, model, limits)
}

fn workspace_limits(workspace: u32) -> AiConcurrencyLimits {
    AiConcurrencyLimits {
        workspace,
        route: 64,
    }
}

async fn fixture(pool: &PgPool) -> (ProjectId, WorkspaceId) {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'AI concurrency fixture')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project fixture inserts");
    let workspace_id: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
        .bind(project_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("project has a workspace");
    sqlx::query("UPDATE ai_usage_rollout_state SET ai_hard_caps_enforced = true WHERE singleton")
        .execute(pool)
        .await
        .expect("test enables the hard-cap admission path");
    sqlx::query("UPDATE ai_usage_backfill_state SET completed_at = now() WHERE singleton")
        .execute(pool)
        .await
        .expect("test marks empty legacy history backfilled");
    (project_id, WorkspaceId::new(workspace_id))
}

async fn set_limits(pool: &PgPool, project_id: ProjectId, workspace_id: WorkspaceId, limit: i64) {
    sqlx::query(
        "UPDATE ai_usage_limits SET limit_micros = $3
         WHERE (scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001')
            OR (scope_kind = 'workspace' AND scope_id = $2)
            OR (scope_kind = 'project' AND scope_id = $1)",
    )
    .bind(project_id.as_uuid())
    .bind(workspace_id.as_uuid())
    .bind(limit)
    .execute(pool)
    .await
    .expect("quota caps update");
    sqlx::query(
        "UPDATE ai_usage_period_buckets SET limit_micros = $3
         WHERE (scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001')
            OR (scope_kind = 'workspace' AND scope_id = $2)
            OR (scope_kind = 'project' AND scope_id = $1)",
    )
    .bind(project_id.as_uuid())
    .bind(workspace_id.as_uuid())
    .bind(limit)
    .execute(pool)
    .await
    .expect("quota buckets follow the fixture caps");
}

/// Live leases of one scope key right now.
async fn live_slots(pool: &PgPool, kind: AiConcurrencyScopeKind, key: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM ai_concurrency_slots
         WHERE scope_kind = $1 AND scope_key = $2 AND lease_expires_at >= clock_timestamp()",
    )
    .bind(kind.as_str())
    .bind(key)
    .fetch_one(pool)
    .await
    .expect("slot rows are counted")
}

fn workspace_key(workspace: WorkspaceId) -> String {
    workspace.as_uuid().to_string()
}

/// A provider that counts calls, optionally waits, optionally fails and can
/// dispatch a nested call from inside its own call.
#[derive(Clone)]
struct FakeProvider {
    calls: Arc<AtomicUsize>,
    delay: Duration,
    failure: Option<String>,
    nested: Option<Arc<dyn AiGateway>>,
    nested_outcomes: Arc<Mutex<Vec<String>>>,
}

impl FakeProvider {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            delay: Duration::ZERO,
            failure: None,
            nested: None,
            nested_outcomes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn failing(message: &str) -> Self {
        Self {
            failure: Some(message.to_owned()),
            ..Self::new()
        }
    }

    fn waiting(delay: Duration) -> Self {
        Self {
            delay,
            ..Self::new()
        }
    }
}

impl AiGateway for FakeProvider {
    fn prepare(&self, request: CompletionRequest) -> Result<PreparedCompletionRequest, AiError> {
        let dialect = ProviderDialect::from_id(&request.route.provider).ok_or_else(|| {
            AiError::UnsupportedUsageRoute {
                provider: request.route.provider.clone(),
                model: request.route.model.clone(),
            }
        })?;
        let body = serde_json::to_vec(&structured_request_body(
            dialect,
            &request.route,
            &request.system_prompt,
            &request.user_prompt,
            &request.evidence,
            &request.schema,
        ))
        .map_err(|_| AiError::UnboundedInput)?;
        Ok(PreparedCompletionRequest::new(request, body))
    }

    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if let Some(nested) = self.nested.as_ref() {
                let outcome = match nested.complete(request).await {
                    Ok(_) => "admitted".to_owned(),
                    Err(error) => format!("{error:?}"),
                };
                self.nested_outcomes
                    .lock()
                    .expect("nested outcomes")
                    .push(outcome);
            }
            if let Some(message) = self.failure.clone() {
                return Err(AiError::Gateway(message));
            }
            Ok(GatewayCompletion {
                output_json: "{}".to_owned(),
                input_tokens: Some(1),
                output_tokens: Some(1),
                reasoning_tokens: Some(0),
                cost_micros: None,
                provider_request_id: None,
                provider_response_id: None,
                served_model: None,
                system_fingerprint: None,
            })
        })
    }
}

fn completion_request(project: ProjectId, workspace: WorkspaceId) -> CompletionRequest {
    CompletionRequest {
        project_id: Some(project),
        usage_context: Some(AiUsageContext {
            workspace_id: Some(workspace),
            principal: Some(UsagePrincipal::Service("concurrency-test".to_owned())),
            run_id: Uuid::new_v4(),
            run_budget_id: Uuid::new_v4(),
            step_id: Some("step:concurrency".to_owned()),
            job_id: None,
            attempt_fence: None,
            run_limit_micros: Some(10_000),
        }),
        route: ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "opencode-go".to_owned(),
            model: "glm-5.3-flash".to_owned(),
            model_version: "glm-5.3-flash".to_owned(),
            parameters: ModelParameters {
                max_tokens: Some(16),
                ..ModelParameters::default()
            },
            route_id: None,
        },
        system_prompt: "system".to_owned(),
        user_prompt: "small request".to_owned(),
        evidence: Vec::new(),
        schema: serde_json::json!({}),
    }
}

/// Counts renewals on top of the real adapter, so a long call's heartbeat is
/// observable.
#[derive(Clone)]
struct CountingAdmission {
    inner: PostgresAiConcurrencyAdmission,
    renewals: Arc<AtomicUsize>,
}

impl AiConcurrencyAdmission for CountingAdmission {
    fn acquire<'a>(&'a self, scope: AiConcurrencyScope) -> AdmissionFuture<'a, SlotLease> {
        self.inner.acquire(scope)
    }

    fn renew<'a>(&'a self, lease: &'a SlotLease) -> AdmissionFuture<'a, ()> {
        self.renewals.fetch_add(1, Ordering::SeqCst);
        self.inner.renew(lease)
    }

    fn release<'a>(&'a self, lease: SlotLease) -> AdmissionFuture<'a, ()> {
        self.inner.release(lease)
    }

    fn renew_interval(&self) -> Duration {
        self.inner.renew_interval()
    }
}

/// An adapter whose leases are always fenced out: every renewal reports lost
/// ownership.
#[derive(Clone)]
struct FencedAdmission {
    inner: PostgresAiConcurrencyAdmission,
}

impl AiConcurrencyAdmission for FencedAdmission {
    fn acquire<'a>(&'a self, scope: AiConcurrencyScope) -> AdmissionFuture<'a, SlotLease> {
        self.inner.acquire(scope)
    }

    fn renew<'a>(&'a self, _lease: &'a SlotLease) -> AdmissionFuture<'a, ()> {
        Box::pin(async { Err(AdmissionError::LeaseLost) })
    }

    fn release<'a>(&'a self, lease: SlotLease) -> AdmissionFuture<'a, ()> {
        self.inner.release(lease)
    }

    fn renew_interval(&self) -> Duration {
        self.inner.renew_interval()
    }
}

#[tokio::test]
async fn a_workspace_race_never_exceeds_its_limit() {
    let Some(pool) = database().await else {
        return;
    };
    // This test asserts admission capacity, not expiry. A 400ms unrenewed
    // lease can make scheduler stalls look like over-grants. Keep holders alive
    // with the production heartbeat, and gate each release explicitly.
    let admission = Arc::new(adapter(&pool, Duration::from_secs(60)));
    let workspace = WorkspaceId::new(Uuid::new_v4());
    let limits = workspace_limits(3);
    let start = Arc::new(tokio::sync::Barrier::new(13));
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (granted_tx, mut granted_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut racers = Vec::new();
    for _ in 0..12 {
        let admission = admission.clone();
        let start = start.clone();
        let live = live.clone();
        let peak = peak.clone();
        let granted_tx = granted_tx.clone();
        racers.push(tokio::spawn(async move {
            start.wait().await;
            loop {
                match admission
                    .acquire(scope(workspace, "opencode-go", "glm-5.3-flash", limits))
                    .await
                {
                    Ok(lease) => {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        assert!(now <= 3, "more than K active holders: {now}");
                        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
                        granted_tx
                            .send(release_tx)
                            .expect("controller observes grant");
                        deepref_ai::renew_while(admission.as_ref(), &lease, release_rx)
                            .await
                            .expect("lease stays live")
                            .expect("controller releases holder");
                        live.fetch_sub(1, Ordering::SeqCst);
                        admission.release(lease).await.expect("lease released");
                        return;
                    }
                    Err(AdmissionError::Saturated { .. }) => {
                        tokio::time::sleep(Duration::from_millis(2)).await
                    }
                    Err(error) => panic!("admission failed: {error}"),
                }
            }
        }));
    }
    start.wait().await;
    tokio::time::timeout(Duration::from_secs(30), async {
        // All K slots must actually be held before releases start. This makes
        // both saturation and the maximum observable on every execution.
        let mut first_wave = Vec::new();
        for _ in 0..3 {
            first_wave.push(granted_rx.recv().await.expect("first wave granted"));
        }
        assert_eq!(live.load(Ordering::SeqCst), 3);
        assert_eq!(
            live_slots(
                &pool,
                AiConcurrencyScopeKind::Workspace,
                &workspace_key(workspace)
            )
            .await,
            3
        );
        assert!(matches!(
            admission
                .acquire(scope(workspace, "opencode-go", "glm-5.3-flash", limits))
                .await,
            Err(AdmissionError::Saturated { .. })
        ));
        for release in first_wave {
            release.send(()).expect("release first wave");
        }
        for _ in 3..12 {
            let release = granted_rx.recv().await.expect("remaining racer admitted");
            assert!(
                live_slots(
                    &pool,
                    AiConcurrencyScopeKind::Workspace,
                    &workspace_key(workspace)
                )
                .await
                    <= 3
            );
            release.send(()).expect("release remaining racer");
        }
        for racer in racers {
            racer.await.expect("every racer completes");
        }
    })
    .await
    .expect("all racers complete as capacity frees");
    assert_eq!(peak.load(Ordering::SeqCst), 3);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace)
        )
        .await,
        0
    );
}

#[tokio::test]
async fn a_saturated_route_refuses_without_holding_the_workspace_slot() {
    let Some(pool) = database().await else {
        return;
    };
    let admission = adapter(&pool, Duration::from_secs(60));
    let limits = AiConcurrencyLimits {
        workspace: 4,
        route: 1,
    };
    let workspace_a = WorkspaceId::new(Uuid::new_v4());
    let workspace_b = WorkspaceId::new(Uuid::new_v4());
    let held = admission
        .acquire(scope(workspace_a, "opencode-go", "glm-5.3-flash", limits))
        .await
        .expect("the first call takes the only route slot");
    // Only one call on that route at a time, across workspaces.
    let refused = admission
        .acquire(scope(workspace_b, "opencode-go", "glm-5.3-flash", limits))
        .await;
    assert!(
        matches!(
            refused,
            Err(AdmissionError::Saturated {
                scope_kind: AiConcurrencyScopeKind::Route
            })
        ),
        "{refused:?}"
    );
    // The all-or-nothing rule: the refused route leaves no workspace slot.
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace_b)
        )
        .await,
        0
    );
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace_a)
        )
        .await,
        1
    );
    // Workspace B keeps its own capacity: another route is admitted at once.
    let other_route = admission
        .acquire(scope(workspace_b, "zai", "glm-5.3", limits))
        .await
        .expect("a free route is admitted immediately");
    admission.release(other_route).await.expect("released");
    admission.release(held).await.expect("released");
    let retried = admission
        .acquire(scope(workspace_b, "opencode-go", "glm-5.3-flash", limits))
        .await
        .expect("the route slot is free again after release");
    admission.release(retried).await.expect("released");
}

#[tokio::test]
async fn a_crashed_holders_slot_is_reclaimed_only_after_expiry() {
    let Some(pool) = database().await else {
        return;
    };
    let crashed = adapter(&pool, SHORT_LEASE);
    let survivor = adapter(&pool, Duration::from_secs(60));
    let target = scope(
        WorkspaceId::new(Uuid::new_v4()),
        "opencode-go",
        "glm-5.3-flash",
        workspace_limits(1),
    );
    let abandoned = crashed
        .acquire(target.clone())
        .await
        .expect("the holder takes the slot");
    // No release: the holder crashes here. Before expiry the slot is not
    // grantable to anyone else.
    let early = survivor.acquire(target.clone()).await;
    assert!(
        matches!(early, Err(AdmissionError::Saturated { .. })),
        "{early:?}"
    );
    tokio::time::sleep(SHORT_LEASE + Duration::from_millis(250)).await;
    let taken = survivor
        .acquire(target.clone())
        .await
        .expect("the expired slot is reusable");
    assert_eq!(taken.workspace.slot_index, abandoned.workspace.slot_index);
    // The takeover is fenced: the crashed holder cannot touch its old lease.
    assert!(matches!(
        crashed.renew(&abandoned).await,
        Err(AdmissionError::LeaseLost)
    ));
    assert!(matches!(
        crashed.release(abandoned).await,
        Err(AdmissionError::LeaseLost)
    ));
    survivor.release(taken).await.expect("released");
}

#[tokio::test]
async fn a_fenced_holder_cannot_renew_or_release_after_takeover() {
    let Some(pool) = database().await else {
        return;
    };
    let old_owner = adapter(&pool, SHORT_LEASE);
    let new_owner = adapter(&pool, Duration::from_secs(60));
    let target = scope(
        WorkspaceId::new(Uuid::new_v4()),
        "opencode-go",
        "glm-5.3-flash",
        AiConcurrencyLimits {
            workspace: 2,
            route: 2,
        },
    );
    let stale = old_owner
        .acquire(target.clone())
        .await
        .expect("the first owner is admitted");
    tokio::time::sleep(SHORT_LEASE + Duration::from_millis(250)).await;
    let taken = new_owner
        .acquire(target.clone())
        .await
        .expect("the second owner takes over both slots");
    // A renewal or release with the stale generation is a no-op that reports
    // lost ownership.
    assert!(matches!(
        old_owner.renew(&stale).await,
        Err(AdmissionError::LeaseLost)
    ));
    assert!(matches!(
        old_owner.release(stale).await,
        Err(AdmissionError::LeaseLost)
    ));
    // The new holder is unaffected.
    new_owner
        .renew(&taken)
        .await
        .expect("the new holder keeps both slots");
    let held_by_new_owner: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_concurrency_slots
         WHERE holder_id = $1 AND lease_expires_at >= clock_timestamp()",
    )
    .bind(new_owner.holder_id())
    .fetch_one(&pool)
    .await
    .expect("holder rows are counted");
    assert_eq!(held_by_new_owner, 2);
    new_owner.release(taken).await.expect("released");
}

#[tokio::test]
async fn lowering_the_limit_drains_slots_beyond_the_new_value() {
    let Some(pool) = database().await else {
        return;
    };
    let admission = adapter(&pool, Duration::from_secs(30));
    let workspace = WorkspaceId::new(Uuid::new_v4());
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(
            admission
                .acquire(scope(
                    workspace,
                    "opencode-go",
                    "glm-5.3-flash",
                    workspace_limits(4),
                ))
                .await
                .expect("the original limit grants four slots"),
        );
    }
    assert!(
        held.iter().all(|lease| lease.workspace.slot_index < 4),
        "grants stay inside the original limit"
    );
    // Lowered limit: nothing is grantable while the old leases are live.
    let lowered = admission
        .acquire(scope(
            workspace,
            "opencode-go",
            "glm-5.3-flash",
            workspace_limits(2),
        ))
        .await;
    assert!(matches!(lowered, Err(AdmissionError::Saturated { .. })));
    // Slots at or above the new limit drain; the lower ones keep running.
    let mut kept = Vec::new();
    for lease in held {
        if lease.workspace.slot_index >= 2 {
            admission.release(lease).await.expect("drained");
        } else {
            kept.push(lease);
        }
    }
    assert_eq!(kept.len(), 2);
    // Two live slots below the new limit: still saturated.
    let still = admission
        .acquire(scope(
            workspace,
            "opencode-go",
            "glm-5.3-flash",
            workspace_limits(2),
        ))
        .await;
    assert!(matches!(still, Err(AdmissionError::Saturated { .. })));
    // Once one drains, a new grant is admitted strictly below the new limit.
    let drained = kept.pop().expect("two leases were kept");
    admission.release(drained).await.expect("drained");
    let fresh = admission
        .acquire(scope(
            workspace,
            "opencode-go",
            "glm-5.3-flash",
            workspace_limits(2),
        ))
        .await
        .expect("capacity below the new limit is grantable");
    assert!(
        fresh.workspace.slot_index < 2,
        "no new grant exceeds the new limit"
    );
    admission.release(fresh).await.expect("released");
    admission.release(kept.pop().expect("one lease")).await.ok();
}

#[tokio::test]
async fn saturation_refuses_before_dispatch_and_releases_the_reservation() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let limits = workspace_limits(1);
    let admission = adapter(&pool, Duration::from_secs(30));
    let held = admission
        .acquire(scope(workspace, "opencode-go", "glm-5.3-flash", limits))
        .await
        .expect("the workspace's only slot is taken");
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = FakeProvider {
        calls: Arc::clone(&calls),
        ..FakeProvider::new()
    };
    let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
        .with_prices(PriceBook::default())
        .with_admission(Arc::new(adapter(&pool, Duration::from_secs(30))), limits);
    let refused = gateway
        .complete(completion_request(project, workspace))
        .await;
    assert!(
        matches!(
            refused,
            Err(AiError::AdmissionSaturated {
                scope_kind: AiConcurrencyScopeKind::Workspace
            })
        ),
        "{refused:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the provider is never called"
    );
    let row = sqlx::query("SELECT state, void_reason FROM ai_usage_reservations")
        .fetch_one(&pool)
        .await
        .expect("the reservation is readable");
    let state: String = row.get("state");
    let void_reason: String = row.get("void_reason");
    assert_eq!(state, "voided", "the reservation is released");
    assert_eq!(void_reason, "concurrency_admission_refused");
    let reserved: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(reserved_micros), 0)::bigint FROM ai_usage_period_buckets",
    )
    .fetch_one(&pool)
    .await
    .expect("buckets are readable");
    assert_eq!(reserved, 0, "the refusal is proven zero liability");
    // The blocked call took no slot of its own.
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace)
        )
        .await,
        1
    );
    admission.release(held).await.expect("released");
}

#[tokio::test]
async fn a_successful_or_failing_call_releases_its_slot() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let limits = workspace_limits(2);
    let admission = Arc::new(adapter(&pool, Duration::from_secs(30)));
    for failing in [false, true] {
        let provider = if failing {
            FakeProvider::failing("provider exploded")
        } else {
            FakeProvider::new()
        };
        let calls = Arc::clone(&provider.calls);
        let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
            .with_prices(PriceBook::default())
            .with_admission(admission.clone(), limits);
        let outcome = gateway
            .complete(completion_request(project, workspace))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            live_slots(
                &pool,
                AiConcurrencyScopeKind::Workspace,
                &workspace_key(workspace)
            )
            .await,
            0,
            "failing={failing}: the slot is released either way"
        );
        assert_eq!(
            live_slots(
                &pool,
                AiConcurrencyScopeKind::Route,
                "opencode-go/glm-5.3-flash"
            )
            .await,
            0
        );
        if failing {
            assert!(outcome.is_err(), "the failing call reports its failure");
        } else {
            assert!(
                outcome.is_ok(),
                "the successful call returns its completion"
            );
        }
    }
}

#[tokio::test]
async fn a_long_call_renews_its_lease_while_it_runs() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let inner = adapter(&pool, SHORT_LEASE);
    let renewals = Arc::new(AtomicUsize::new(0));
    let counting = CountingAdmission {
        inner,
        renewals: Arc::clone(&renewals),
    };
    let provider = FakeProvider::waiting(SHORT_LEASE + Duration::from_millis(250));
    let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
        .with_prices(PriceBook::default())
        .with_admission(
            Arc::new(counting),
            AiConcurrencyLimits {
                workspace: 2,
                route: 2,
            },
        );
    let completion = gateway
        .complete(completion_request(project, workspace))
        .await;
    assert!(
        completion.is_ok(),
        "a call longer than its lease still completes: {completion:?}"
    );
    assert!(
        renewals.load(Ordering::SeqCst) >= 2,
        "the lease heartbeat ran during the call: {}",
        renewals.load(Ordering::SeqCst)
    );
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace)
        )
        .await,
        0
    );
}

#[tokio::test]
async fn a_lost_lease_stops_the_call_and_holds_its_liability() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let fenced = FencedAdmission {
        inner: adapter(&pool, SHORT_LEASE),
    };
    let provider = FakeProvider::waiting(SHORT_LEASE + Duration::from_millis(250));
    let calls = Arc::clone(&provider.calls);
    let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
        .with_prices(PriceBook::default())
        .with_admission(Arc::new(fenced), workspace_limits(2));
    let stopped = gateway
        .complete(completion_request(project, workspace))
        .await;
    assert!(
        matches!(stopped, Err(AiError::AdmissionLeaseLost)),
        "{stopped:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the call was under way");
    let row = sqlx::query("SELECT state FROM ai_usage_reservations")
        .fetch_one(&pool)
        .await
        .expect("the reservation is readable");
    let state: String = row.get("state");
    // The interrupted call's usage is unknown: its liability is held for
    // reconciliation, never released as zero liability.
    assert_eq!(state, "unresolved");
}

#[tokio::test]
async fn a_nested_call_fails_fast_under_saturation_without_deadlock() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    // The workspace admits exactly one call at a time: the parent holds the
    // last slot while its nested call runs.
    let limits = workspace_limits(1);
    let admission = Arc::new(adapter(&pool, Duration::from_secs(30)));
    let nested_outcomes = Arc::new(Mutex::new(Vec::new()));
    let mut provider = FakeProvider {
        nested_outcomes: Arc::clone(&nested_outcomes),
        ..FakeProvider::new()
    };
    let inner_provider = FakeProvider::new();
    let nested_calls = Arc::clone(&inner_provider.calls);
    let nested_gateway: Arc<dyn AiGateway> = Arc::new(
        MeteredGateway::new(inner_provider, Arc::new(PostgresUsageLedger::new(&pool)))
            .with_prices(PriceBook::default())
            .with_admission(admission.clone(), limits),
    );
    provider.nested = Some(nested_gateway);
    let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
        .with_prices(PriceBook::default())
        .with_admission(admission.clone(), limits);
    // The nested call happens inside the parent's provider call, while the
    // parent still holds the last slot. It must fail fast and never block.
    let finished = tokio::time::timeout(
        Duration::from_secs(10),
        gateway.complete(completion_request(project, workspace)),
    )
    .await
    .expect("the parent call never deadlocks on its nested call");
    assert!(
        finished.is_ok(),
        "the parent call itself completes: {finished:?}"
    );
    assert_eq!(
        nested_calls.load(Ordering::SeqCst),
        0,
        "the nested call never reaches its provider"
    );
    let outcomes = nested_outcomes.lock().expect("nested outcomes").clone();
    assert_eq!(outcomes.len(), 1, "the nested call was attempted once");
    assert!(
        outcomes[0].contains("AdmissionSaturated"),
        "the nested call failed fast with saturation: {}",
        outcomes[0]
    );
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace)
        )
        .await,
        0,
        "the parent's slot is released after the call"
    );
}

#[tokio::test]
async fn two_instances_share_one_limit() {
    let url = match deepref_app::test_isolation::isolated_url_from_env("TEST_DATABASE_URL").await {
        Some(url) => Some(url),
        None => deepref_app::test_isolation::isolated_url_from_env("DATABASE_URL").await,
    };
    let Some(url) = url else {
        return;
    };
    let pool_a = second_pool(&url).await;
    let pool_b = second_pool(&url).await;
    let admission_a = adapter(&pool_a, Duration::from_secs(60));
    let admission_b = adapter(&pool_b, Duration::from_secs(60));
    assert_ne!(admission_a.holder_id(), admission_b.holder_id());
    let target = scope(
        WorkspaceId::new(Uuid::new_v4()),
        "opencode-go",
        "glm-5.3-flash",
        workspace_limits(1),
    );
    let held = admission_a
        .acquire(target.clone())
        .await
        .expect("instance A is admitted");
    let refused = admission_b.acquire(target.clone()).await;
    assert!(
        matches!(refused, Err(AdmissionError::Saturated { .. })),
        "{refused:?}"
    );
    admission_a.release(held).await.expect("released");
    let after = admission_b
        .acquire(target.clone())
        .await
        .expect("instance B takes the freed slot");
    admission_b.release(after).await.expect("released");
}

#[tokio::test]
async fn long_expired_rows_are_swept_and_recent_ones_are_kept() {
    let Some(pool) = database().await else {
        return;
    };
    let workspace = WorkspaceId::new(Uuid::new_v4());
    let key = workspace_key(workspace);
    for age_minutes in [120_i64, 1] {
        sqlx::query(
            "INSERT INTO ai_concurrency_slots
               (scope_kind, scope_key, slot_index, holder_id, generation, acquired_at, lease_expires_at)
             VALUES ('workspace', $1, $2, $3, 1, now(), now() - make_interval(mins => $4::integer))",
        )
        .bind(&key)
        .bind(age_minutes)
        .bind(Uuid::new_v4())
        .bind(age_minutes)
        .execute(&pool)
        .await
        .expect("expired rows are inserted");
    }
    sqlx::query(
        "INSERT INTO ai_concurrency_slots
           (scope_kind, scope_key, slot_index, holder_id, generation, acquired_at, lease_expires_at)
         VALUES ('workspace', $1, 7, $2, 1, now(), now() + interval '1 minute')",
    )
    .bind(&key)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("a live row is inserted");
    let deleted = sweep_ai_concurrency_slots(&pool)
        .await
        .expect("the sweep runs");
    assert_eq!(deleted, 1, "only the long-expired row is deleted");
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_concurrency_slots WHERE scope_key = $1")
            .bind(&key)
            .fetch_one(&pool)
            .await
            .expect("rows are counted");
    assert_eq!(remaining, 2);
}

#[tokio::test]
async fn an_expired_lease_cannot_be_resurrected_without_takeover() {
    let Some(pool) = database().await else {
        return;
    };
    let admission = adapter(&pool, Duration::from_secs(60));
    let lease = admission
        .acquire(scope(
            WorkspaceId::new(Uuid::new_v4()),
            "test",
            "expiry",
            workspace_limits(1),
        ))
        .await
        .expect("acquired");
    sqlx::query(
        "UPDATE ai_concurrency_slots SET lease_expires_at = clock_timestamp() - interval '1 second'
                 WHERE holder_id = $1",
    )
    .bind(admission.holder_id())
    .execute(&pool)
    .await
    .expect("expire both slots");
    assert!(
        matches!(
            admission.renew(&lease).await,
            Err(AdmissionError::LeaseLost)
        ),
        "expiry itself fences renewal, even before takeover"
    );
}

#[tokio::test]
async fn cancelling_a_dispatched_call_releases_capacity_and_retains_liability() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let admission = Arc::new(adapter(&pool, Duration::from_secs(60)));
    let provider = FakeProvider::waiting(Duration::from_secs(120));
    let calls = provider.calls.clone();
    let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
        .with_admission(admission, workspace_limits(1));
    let task = tokio::spawn(async move {
        gateway
            .complete(completion_request(project, workspace))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider dispatched");
    task.abort();
    assert!(task.await.expect_err("call was cancelled").is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        while live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(workspace),
        )
        .await
            != 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancellation releases capacity before its 60 second expiry");
    let state: String = sqlx::query_scalar("SELECT state FROM ai_usage_reservations")
        .fetch_one(&pool)
        .await
        .expect("reservation remains");
    assert!(
        state == "open" || state == "unresolved",
        "cancelled provider liability remains held: {state}"
    );
}

#[tokio::test]
async fn locked_claims_refuse_promptly_and_sweeps_skip_renewing_rows() {
    let Some(pool) = database().await else {
        return;
    };
    let admission = adapter(&pool, Duration::from_secs(60));
    let target = scope(
        WorkspaceId::new(Uuid::new_v4()),
        "test",
        "locked",
        workspace_limits(1),
    );
    let old = admission.acquire(target.clone()).await.expect("acquired");
    sqlx::query(
        "UPDATE ai_concurrency_slots SET lease_expires_at = clock_timestamp() - interval '2 hours'",
    )
    .execute(&pool)
    .await
    .expect("expire slots");
    let mut renewing = pool.begin().await.expect("renew transaction");
    sqlx::query("UPDATE ai_concurrency_slots SET lease_expires_at = clock_timestamp() + interval '1 minute'").execute(&mut *renewing).await.expect("renew holds row locks");
    let contender = adapter(&pool, Duration::from_secs(60));
    let denied = tokio::time::timeout(Duration::from_secs(1), contender.acquire(target))
        .await
        .expect("admission never waits on held capacity");
    assert!(
        matches!(denied, Err(AdmissionError::Saturated { .. })),
        "{denied:?}"
    );
    let swept = tokio::time::timeout(Duration::from_secs(1), sweep_ai_concurrency_slots(&pool))
        .await
        .expect("sweep skips locked rows")
        .expect("swept");
    assert_eq!(swept, 0);
    renewing.commit().await.expect("renew commits");
    assert_eq!(
        live_slots(
            &pool,
            AiConcurrencyScopeKind::Workspace,
            &workspace_key(old.scope.workspace_id)
        )
        .await,
        1,
        "renewed row survives sweep"
    );
    admission.release(old).await.expect("released");
}

#[tokio::test]
async fn another_instance_cannot_renew_or_release_a_live_owners_lease() {
    let Some(pool) = database().await else {
        return;
    };
    let owner = adapter(&pool, Duration::from_secs(60));
    let other = adapter(&pool, Duration::from_secs(60));
    let lease = owner
        .acquire(scope(
            WorkspaceId::new(Uuid::new_v4()),
            "test",
            "owner",
            workspace_limits(1),
        ))
        .await
        .expect("acquired");
    assert!(matches!(
        other.renew(&lease).await,
        Err(AdmissionError::LeaseLost)
    ));
    assert!(matches!(
        other.release(lease.clone()).await,
        Err(AdmissionError::LeaseLost)
    ));
    owner
        .renew(&lease)
        .await
        .expect("original lease unaffected");
    owner.release(lease).await.expect("released");
}

#[tokio::test]
async fn disabled_hard_caps_never_acquire_slots_or_dispatch() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    sqlx::query("UPDATE ai_usage_rollout_state SET ai_hard_caps_enforced = false WHERE singleton")
        .execute(&pool)
        .await
        .expect("default disabled gate");
    let provider = FakeProvider::new();
    let calls = provider.calls.clone();
    let gateway = MeteredGateway::new(provider, Arc::new(PostgresUsageLedger::new(&pool)))
        .with_admission(
            Arc::new(adapter(&pool, Duration::from_secs(60))),
            workspace_limits(1),
        );
    assert!(matches!(
        gateway
            .complete(completion_request(project, workspace))
            .await,
        Err(AiError::HardCapsDisabled)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let slots: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_concurrency_slots")
        .fetch_one(&pool)
        .await
        .expect("slot count");
    assert_eq!(slots, 0);
}

#[tokio::test]
async fn deleted_and_recreated_slots_never_reuse_their_fence() {
    let Some(pool) = database().await else {
        return;
    };
    let admission = adapter(&pool, Duration::from_secs(60));
    let target = scope(
        WorkspaceId::new(Uuid::new_v4()),
        "test",
        "recreate",
        workspace_limits(1),
    );
    let old = admission.acquire(target.clone()).await.expect("acquired");
    admission.release(old.clone()).await.expect("released");
    let new = admission.acquire(target).await.expect("recreated");
    assert!(new.workspace.generation > old.workspace.generation);
    assert!(new.route.generation > old.route.generation);
    assert!(matches!(
        admission.renew(&old).await,
        Err(AdmissionError::LeaseLost)
    ));
    assert!(matches!(
        admission.release(old).await,
        Err(AdmissionError::LeaseLost)
    ));
    admission.renew(&new).await.expect("new lease unaffected");
    admission.release(new).await.expect("released");
    assert_eq!(admission.metrics().snapshot().grants.workspace, 2);
}
