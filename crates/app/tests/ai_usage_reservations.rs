#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::panic_in_result_fn
)]

//! Real PostgreSQL concurrency and recovery checks for AI usage reservations.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use deepref_ai::{
    AiError, AiFuture, AiGateway, AiUsageContext, CompletionRequest, GatewayCompletion,
    MeteredGateway, ModelParameters, ModelPrice, ModelProfile, PriceBook, ResolvedModel,
    UsageLedger, UsageObservation, UsagePriceSnapshot, UsagePrincipal, UsageReservationRequest,
    UsageTopUpRequest, ZeroLiabilityProof,
};
use deepref_app::{
    AiUsageLimitScope, PostgresUsageLedger, set_ai_usage_cap, waive_ai_usage_overage,
};
use deepref_application::jobs::JobScope;
use deepref_domain::{JobKind, ProjectId, UserId, WorkloadClass, WorkspaceId};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Clone)]
struct CountingGateway(Arc<AtomicUsize>);

impl AiGateway for CountingGateway {
    fn complete<'a>(&'a self, _request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(GatewayCompletion {
                output_json: "{}".to_owned(),
                input_tokens: 1,
                output_tokens: 1,
                reasoning_tokens: 0,
                cost_micros: None,
                served_model: None,
                system_fingerprint: None,
            })
        })
    }
}

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 24).await
}

async fn fixture(pool: &PgPool) -> (ProjectId, WorkspaceId) {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'AI usage reservation fixture')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project fixture inserts");
    let workspace_id: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
        .bind(project_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("project has a workspace");
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
}

fn request(
    project_id: ProjectId,
    run_id: Uuid,
    input_bound: u64,
    output_cap: u64,
    run_limit_micros: i64,
) -> UsageReservationRequest {
    UsageReservationRequest {
        attribution: AiUsageContext {
            workspace_id: None,
            principal: Some(UsagePrincipal::Service("reservation-test".to_owned())),
            run_id,
            step_id: Some("step:test".to_owned()),
            job_id: None,
            attempt_fence: None,
            run_limit_micros: Some(run_limit_micros),
        },
        project_id: Some(project_id),
        profile: "reasoning".to_owned(),
        provider: "test-provider".to_owned(),
        model: "test-model".to_owned(),
        purpose: "structured".to_owned(),
        input_token_bound: Some(input_bound),
        max_output_tokens: Some(output_cap),
        max_reasoning_tokens: None,
        other_cost_micros: 0,
        rounding_margin_micros: 0,
        price: UsagePriceSnapshot {
            version: "test-price-v1".to_owned(),
            input_micros_per_million_tokens: 1_000_000,
            output_micros_per_million_tokens: 1_000_000,
            reasoning_micros_per_million_tokens: None,
        },
    }
}

#[test]
fn reservation_and_settlement_use_each_captured_token_rate() {
    let price = UsagePriceSnapshot {
        version: "rate-test-v1".to_owned(),
        input_micros_per_million_tokens: 1_000_000,
        output_micros_per_million_tokens: 2_000_000,
        reasoning_micros_per_million_tokens: Some(3_000_000),
    };
    assert_eq!(
        price
            .maximum_cost_micros(Some(100), Some(10), Some(5), 4, 2)
            .expect("all bounded components are priced"),
        141
    );
    assert_eq!(
        price
            .actual_cost_micros(&UsageObservation {
                input_tokens: 40,
                output_tokens: 3,
                reasoning_tokens: 2,
                commercial_units: None,
            })
            .expect("provider usage settles at its captured rates"),
        52
    );
    assert!(matches!(
        price.maximum_cost_micros(None, Some(1), Some(1), 0, 0),
        Err(AiError::UnboundedInput)
    ));
    assert!(matches!(
        price.maximum_cost_micros(Some(1), None, Some(1), 0, 0),
        Err(AiError::UnboundedOutput)
    ));
}

#[test]
fn concurrent_index_migrations_are_marked_no_transaction() {
    for version in [123, 125, 126] {
        let index_migration = deepref_app::MIGRATOR
            .iter()
            .find(|migration| migration.version == version)
            .expect("each concurrent reservation index has its own migration");
        assert!(index_migration.no_tx, "migration {version}");
    }
}

async fn bucket(
    pool: &PgPool,
    kind: &str,
    scope_id: Uuid,
    period_start: chrono::NaiveDate,
) -> (i64, i64, i64) {
    let row = sqlx::query(
        "SELECT limit_micros, spent_micros, reserved_micros
         FROM ai_usage_period_buckets WHERE scope_kind = $1 AND scope_id = $2 AND period_start = $3",
    )
    .bind(kind)
    .bind(scope_id)
    .bind(period_start)
    .fetch_one(pool)
    .await
    .expect("quota bucket exists");
    (
        row.get("limit_micros"),
        row.get("spent_micros"),
        row.get("reserved_micros"),
    )
}

#[tokio::test]
async fn concurrent_reservations_never_exceed_any_parent_or_project_limit() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 500).await;
    let ledger = Arc::new(PostgresUsageLedger::new(&pool));
    let run_id = Uuid::new_v4();
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let ledger = Arc::clone(&ledger);
        calls.spawn(async move { ledger.reserve(request(project, run_id, 100, 0, 500)).await });
    }
    let mut accepted = Vec::new();
    let mut denied = 0;
    while let Some(result) = calls.join_next().await {
        match result.expect("reservation task joins") {
            Ok(reservation) => accepted.push(reservation),
            Err(AiError::UsageLimitExceeded) => denied += 1,
            Err(error) => panic!("unexpected reservation error: {error}"),
        }
    }
    assert_eq!(accepted.len(), 5);
    assert_eq!(denied, 7);
    let period = accepted[0].period_start;
    for (kind, id) in [
        ("platform", Uuid::from_u128(1)),
        ("workspace", workspace.as_uuid()),
        ("project", project.as_uuid()),
        ("run", run_id),
    ] {
        let (limit, spent, reserved) = bucket(&pool, kind, id, period).await;
        assert_eq!(limit, 500, "{kind} cap");
        assert!(spent + reserved <= limit, "{kind} oversubscribed");
        assert_eq!(
            reserved, 500,
            "accepted bounds are atomically held at {kind}"
        );
    }
}

#[tokio::test]
async fn competing_top_ups_reserve_the_last_headroom_atomically() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 1_000).await;
    let ledger = Arc::new(PostgresUsageLedger::new(&pool));
    let run_id = Uuid::new_v4();
    let reservation = ledger
        .reserve(request(project, run_id, 100, 0, 1_000))
        .await
        .expect("initial reservation");
    assert_eq!(reservation.reserved_cost_micros, 100);

    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let ledger = Arc::clone(&ledger);
        let reservation = reservation.clone();
        calls.spawn(async move {
            ledger
                .top_up(
                    &reservation,
                    UsageTopUpRequest {
                        additional_input_token_bound: 500,
                        additional_output_tokens: 0,
                        additional_reasoning_tokens: 0,
                        other_cost_micros: 0,
                        rounding_margin_micros: 0,
                    },
                )
                .await
        });
    }
    let mut accepted = 0;
    let mut denied = 0;
    while let Some(result) = calls.join_next().await {
        match result.expect("top-up task joins") {
            Ok(updated) => {
                accepted += 1;
                assert_eq!(updated.reserved_cost_micros, 600);
            }
            Err(AiError::UsageLimitExceeded) => denied += 1,
            Err(error) => panic!("unexpected top-up error: {error}"),
        }
    }
    assert_eq!(accepted, 1);
    assert_eq!(denied, 1);
    for (kind, id) in [
        ("platform", Uuid::from_u128(1)),
        ("workspace", workspace.as_uuid()),
        ("project", project.as_uuid()),
        ("run", run_id),
    ] {
        assert_eq!(
            bucket(&pool, kind, id, reservation.period_start).await.2,
            600
        );
    }
}

#[tokio::test]
async fn child_project_limit_cannot_bypass_its_workspace_or_platform_cap() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    sqlx::query(
        "UPDATE ai_usage_limits SET limit_micros = CASE scope_kind
           WHEN 'project' THEN 10_000 ELSE 500 END
         WHERE (scope_kind = 'platform' AND scope_id = $3)
            OR (scope_kind = 'workspace' AND scope_id = $2)
            OR (scope_kind = 'project' AND scope_id = $1)",
    )
    .bind(project.as_uuid())
    .bind(workspace.as_uuid())
    .bind(Uuid::from_u128(1))
    .execute(&pool)
    .await
    .expect("child has more room than both parents");
    let ledger = PostgresUsageLedger::new(&pool);
    assert!(matches!(
        ledger
            .reserve(request(project, Uuid::new_v4(), 501, 0, 10_000))
            .await,
        Err(AiError::UsageLimitExceeded)
    ));
}

#[tokio::test]
async fn partial_usage_above_its_hold_records_true_cost_and_blocks_workspace() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let ledger = PostgresUsageLedger::new(&pool);
    let reservation = ledger
        .reserve(request(project, Uuid::new_v4(), 100, 0, 1_000))
        .await
        .expect("reservation opens");
    ledger
        .unresolved(
            &reservation,
            Some(UsageObservation {
                input_tokens: 150,
                output_tokens: 0,
                reasoning_tokens: 0,
                commercial_units: None,
            }),
            "partial_stream_exceeded_bound",
        )
        .await
        .expect("known partial overage is retained");
    let row = sqlx::query(
        "SELECT state, recognized_cost_micros, overage_micros FROM ai_usage_reservations WHERE id=$1",
    )
    .bind(reservation.id)
    .fetch_one(&pool)
    .await
    .expect("unresolved overage record");
    assert_eq!(row.get::<String, _>("state"), "unresolved");
    assert_eq!(row.get::<i64, _>("recognized_cost_micros"), 150);
    assert_eq!(row.get::<i64, _>("overage_micros"), 50);
    let (_, spent, reserved) = bucket(
        &pool,
        "project",
        project.as_uuid(),
        reservation.period_start,
    )
    .await;
    assert_eq!(spent, 150);
    assert_eq!(reserved, 0);
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM ai_usage_workspace_blocks WHERE workspace_id=$1 AND cleared_at IS NULL)",
    )
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("workspace block exists"));
}

#[tokio::test]
async fn large_input_is_denied_before_provider_dispatch_and_missing_output_cap_is_typed() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 1_000).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let prices = PriceBook::new([(
        "test-provider".to_owned(),
        "test-model".to_owned(),
        ModelPrice {
            input_micros_per_million_tokens: 1_000_000,
            output_micros_per_million_tokens: 0,
            reasoning_micros_per_million_tokens: None,
        },
    )]);
    let gateway = MeteredGateway::new(
        CountingGateway(Arc::clone(&calls)),
        Arc::new(PostgresUsageLedger::new(&pool)),
    )
    .with_prices(prices);
    let mut parameters = ModelParameters {
        max_tokens: Some(1),
        ..ModelParameters::default()
    };
    let route = ResolvedModel {
        profile: ModelProfile::Reasoning,
        provider: "test-provider".to_owned(),
        model: "test-model".to_owned(),
        model_version: "v1".to_owned(),
        parameters: parameters.clone(),
        route_id: None,
    };
    let context = AiUsageContext {
        workspace_id: Some(workspace),
        principal: Some(UsagePrincipal::Service("large-prompt-test".to_owned())),
        run_id: Uuid::new_v4(),
        step_id: Some("large-prompt".to_owned()),
        job_id: None,
        attempt_fence: None,
        run_limit_micros: Some(1_000),
    };
    let large = CompletionRequest {
        project_id: Some(project),
        usage_context: Some(context.clone()),
        route: route.clone(),
        system_prompt: "system".to_owned(),
        user_prompt: "x".repeat(5_000),
        evidence: Vec::new(),
        schema: serde_json::json!({}),
    };
    assert!(matches!(
        gateway.complete(large).await,
        Err(AiError::UsageLimitExceeded)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    parameters.max_tokens = None;
    let unbounded = CompletionRequest {
        project_id: Some(project),
        usage_context: Some(context),
        route: ResolvedModel {
            parameters,
            ..route
        },
        system_prompt: "system".to_owned(),
        user_prompt: "small".to_owned(),
        evidence: Vec::new(),
        schema: serde_json::json!({}),
    };
    assert!(matches!(
        gateway.complete(unbounded).await,
        Err(AiError::UnboundedOutput)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn partial_usage_zero_liability_release_settlement_and_overage_are_distinct() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let ledger = PostgresUsageLedger::new(&pool);

    let voided = ledger
        .reserve(request(project, Uuid::new_v4(), 100, 0, 1_000))
        .await
        .expect("zero-liability reservation");
    ledger
        .void_zero_liability(
            &voided,
            ZeroLiabilityProof::ProviderRejected { status: 400 },
        )
        .await
        .expect("explicit provider rejection releases the hold");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM ai_usage_reservations WHERE id=$1")
            .bind(voided.id)
            .fetch_one(&pool)
            .await
            .expect("void state"),
        "voided"
    );
    assert_eq!(
        bucket(&pool, "project", project.as_uuid(), voided.period_start)
            .await
            .2,
        0
    );

    let partial = ledger
        .reserve(request(project, Uuid::new_v4(), 200, 0, 1_000))
        .await
        .expect("partial reservation");
    ledger
        .unresolved(
            &partial,
            Some(UsageObservation {
                input_tokens: 50,
                output_tokens: 0,
                reasoning_tokens: 0,
                commercial_units: None,
            }),
            "provider_stream_interrupted",
        )
        .await
        .expect("known partial usage is committed and remaining hold stays open");
    let (_, spent, reserved) =
        bucket(&pool, "project", project.as_uuid(), partial.period_start).await;
    assert_eq!(spent, 50);
    assert_eq!(reserved, 150);
    let partial_row =
        sqlx::query("SELECT state, recognized_cost_micros FROM ai_usage_reservations WHERE id=$1")
            .bind(partial.id)
            .fetch_one(&pool)
            .await
            .expect("partial usage record");
    assert_eq!(partial_row.get::<String, _>("state"), "unresolved");
    assert_eq!(partial_row.get::<i64, _>("recognized_cost_micros"), 50);

    let settled = ledger
        .reserve(request(project, Uuid::new_v4(), 100, 0, 1_000))
        .await
        .expect("settlement reservation");
    let actual = UsageObservation {
        input_tokens: 40,
        output_tokens: 0,
        reasoning_tokens: 0,
        commercial_units: None,
    };
    ledger
        .observe(&settled, actual.clone())
        .await
        .expect("observation persists first");
    ledger
        .settle(&settled, actual, "test-price-v1")
        .await
        .expect("known usage settles");
    let (_, spent, reserved) =
        bucket(&pool, "project", project.as_uuid(), settled.period_start).await;
    assert_eq!(spent, 90);
    assert_eq!(reserved, 150);
    let service_attribution = sqlx::query(
        "SELECT principal_kind, principal_id FROM ai_usage_ledger WHERE reservation_id=$1",
    )
    .bind(settled.id)
    .fetch_one(&pool)
    .await
    .expect("settled service usage fact");
    assert_eq!(
        service_attribution.get::<String, _>("principal_kind"),
        "service"
    );
    assert_eq!(
        service_attribution.get::<String, _>("principal_id"),
        "reservation-test"
    );

    let overage = ledger
        .reserve(request(project, Uuid::new_v4(), 100, 0, 1_000))
        .await
        .expect("overage reservation");
    let actual = UsageObservation {
        input_tokens: 150,
        output_tokens: 0,
        reasoning_tokens: 0,
        commercial_units: None,
    };
    ledger
        .observe(&overage, actual.clone())
        .await
        .expect("overage observed");
    ledger
        .settle(&overage, actual, "test-price-v1")
        .await
        .expect("true provider cost is recorded");
    let block = sqlx::query(
        "SELECT overage_micros FROM ai_usage_workspace_blocks WHERE workspace_id=$1 AND cleared_at IS NULL",
    )
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("workspace is blocked");
    assert_eq!(block.get::<i64, _>("overage_micros"), 50);
    assert!(matches!(
        ledger
            .reserve(request(project, Uuid::new_v4(), 1, 0, 1_000))
            .await,
        Err(AiError::WorkspaceAiBlocked)
    ));
}

#[tokio::test]
async fn failed_ledger_write_recovers_only_after_job_lease_is_gone_and_fenced() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 10_000).await;
    let job_id = Uuid::new_v4();
    let enqueued = deepref_app::scoped_job(
        job_id,
        JobScope::Workspace(workspace),
        JobKind::AiUsageReconcile,
        serde_json::json!({}),
        format!("usage-recovery-test:{job_id}"),
    );
    deepref_app::enqueue_job_pool(&pool, &enqueued)
        .await
        .expect("workspace recovery job enqueues");
    let claimed = deepref_app::claim_jobs(
        &pool,
        "usage-test-worker",
        Duration::from_secs(60),
        &[WorkloadClass::Ai],
        1,
    )
    .await
    .expect("workspace recovery job claims")
    .pop()
    .expect("one AI job claims");
    assert_eq!(claimed.id, job_id);
    let lease = claimed.lease("usage-test-worker");
    let ledger = PostgresUsageLedger::new(&pool);
    let principal_id = Uuid::new_v4();
    let mut call = request(project, Uuid::new_v4(), 100, 0, 1_000);
    call.attribution.job_id = Some(lease.id);
    call.attribution.attempt_fence = Some(lease.claim_seq);
    call.attribution.principal = Some(UsagePrincipal::User(UserId::new(principal_id)));
    let reservation = ledger
        .reserve(call)
        .await
        .expect("fenced reservation opens");
    let observation = UsageObservation {
        input_tokens: 25,
        output_tokens: 0,
        reasoning_tokens: 0,
        commercial_units: None,
    };
    ledger
        .observe(&reservation, observation.clone())
        .await
        .expect("provider evidence is persisted before settlement");

    let previous_period: chrono::NaiveDate = sqlx::query_scalar(
        "SELECT (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '1 month')::date",
    )
    .fetch_one(&pool)
    .await
    .expect("previous UTC month");
    sqlx::query("UPDATE ai_usage_reservations SET period_start=$2 WHERE id=$1")
        .bind(reservation.id)
        .bind(previous_period)
        .execute(&pool)
        .await
        .expect("fixture moves the charge into its captured month");
    sqlx::query(
        "UPDATE ai_usage_period_buckets SET period_start=$3
         WHERE period_start=$2 AND
           ((scope_kind='platform' AND scope_id=$4)
            OR (scope_kind='workspace' AND scope_id=$5)
            OR (scope_kind='project' AND scope_id=$6)
            OR (scope_kind='run' AND scope_id=(SELECT run_id FROM ai_usage_reservations WHERE id=$1)))",
    )
    .bind(reservation.id)
    .bind(reservation.period_start)
    .bind(previous_period)
    .bind(Uuid::from_u128(1))
    .bind(workspace.as_uuid())
    .bind(project.as_uuid())
    .execute(&pool)
    .await
    .expect("all hierarchy buckets move with the reservation's captured month");

    sqlx::query(
        "CREATE FUNCTION fail_ai_usage_fact_for_test() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected AI ledger failure'; END $$",
    )
    .execute(&pool)
    .await
    .expect("failure trigger function");
    sqlx::query(
        "CREATE TRIGGER fail_ai_usage_fact_for_test BEFORE INSERT ON ai_usage_ledger
         FOR EACH ROW WHEN (NEW.reservation_id IS NOT NULL)
         EXECUTE FUNCTION fail_ai_usage_fact_for_test()",
    )
    .execute(&pool)
    .await
    .expect("failure trigger");
    assert!(
        ledger
            .settle(&reservation, observation, "test-price-v1")
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM ai_usage_reservations WHERE id=$1")
            .bind(reservation.id)
            .fetch_one(&pool)
            .await
            .expect("failed transaction left observation recoverable"),
        "observed"
    );
    sqlx::query("DROP TRIGGER fail_ai_usage_fact_for_test ON ai_usage_ledger")
        .execute(&pool)
        .await
        .expect("drop failure trigger");
    sqlx::query("DROP FUNCTION fail_ai_usage_fact_for_test()")
        .execute(&pool)
        .await
        .expect("drop failure function");
    let leased_report =
        deepref_app::reconcile_ai_usage_workspace(&pool, workspace, "unavailable-at-recovery")
            .await
            .expect("reconciler checks a live lease");
    assert_eq!(leased_report.settled_from_observation, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM ai_usage_reservations WHERE id=$1")
            .bind(reservation.id)
            .fetch_one(&pool)
            .await
            .expect("live-lease reservation remains open"),
        "observed"
    );
    assert!(
        deepref_app::complete_job(&pool, &lease)
            .await
            .expect("job completes and clears its lease")
    );

    let report =
        deepref_app::reconcile_ai_usage_workspace(&pool, workspace, "unavailable-at-recovery")
            .await
            .expect("stale observed reservation recovers");
    assert_eq!(report.settled_from_observation, 1);
    let fact = sqlx::query(
        "SELECT l.period_start, l.price_version, l.current_price_version, l.pricing_decision,
                l.principal_kind, l.principal_id, r.state
         FROM ai_usage_ledger l JOIN ai_usage_reservations r ON r.id=l.reservation_id
         WHERE l.reservation_id=$1",
    )
    .bind(reservation.id)
    .fetch_one(&pool)
    .await
    .expect("recovered usage fact");
    assert_eq!(
        fact.get::<chrono::NaiveDate, _>("period_start"),
        previous_period
    );
    assert_eq!(fact.get::<String, _>("price_version"), "test-price-v1");
    assert_eq!(
        fact.get::<String, _>("current_price_version"),
        "unavailable-at-recovery"
    );
    assert_eq!(
        fact.get::<String, _>("pricing_decision"),
        "reservation_snapshot_recovery"
    );
    assert_eq!(
        fact.get::<Option<String>, _>("principal_kind").as_deref(),
        Some("user")
    );
    assert_eq!(
        fact.get::<Option<String>, _>("principal_id").as_deref(),
        Some(principal_id.to_string().as_str())
    );
    assert_eq!(fact.get::<String, _>("state"), "settled");
    let fact_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ai_usage_ledger WHERE reservation_id=$1 AND period_start=$2",
    )
    .bind(reservation.id)
    .bind(previous_period)
    .fetch_one(&pool)
    .await
    .expect("usage has one captured billing period");
    assert_eq!(fact_count, 1);
    assert_eq!(
        bucket(&pool, "project", project.as_uuid(), previous_period)
            .await
            .1,
        25
    );
}

#[tokio::test]
async fn migration_backfill_preserves_project_caps_and_sets_parent_sums() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    let project_limit: i64 = sqlx::query_scalar(
        "SELECT limit_micros FROM ai_usage_limits WHERE scope_kind='project' AND scope_id=$1",
    )
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("project sub-limit exists");
    let project_budget: i64 =
        sqlx::query_scalar("SELECT ai_monthly_budget_micros FROM projects WHERE id=$1")
            .bind(project.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("legacy project allowance remains available");
    assert_eq!(project_limit, project_budget);
    let workspace_limit: i64 = sqlx::query_scalar(
        "SELECT limit_micros FROM ai_usage_limits WHERE scope_kind='workspace' AND scope_id=$1",
    )
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("workspace parent cap exists");
    let workspace_children: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(limit_micros), 0)::bigint FROM ai_usage_limits l
         JOIN projects p ON p.id=l.scope_id
         WHERE l.scope_kind='project' AND p.workspace_id=$1",
    )
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("workspace child caps sum");
    assert_eq!(workspace_limit, workspace_children);
    let platform_limit: i64 = sqlx::query_scalar(
        "SELECT limit_micros FROM ai_usage_limits WHERE scope_kind='platform' AND scope_id=$1",
    )
    .bind(Uuid::from_u128(1))
    .fetch_one(&pool)
    .await
    .expect("platform parent cap exists");
    let workspace_caps: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(limit_micros), 0)::bigint FROM ai_usage_limits WHERE scope_kind='workspace'",
    )
    .fetch_one(&pool)
    .await
    .expect("workspace caps sum");
    assert_eq!(platform_limit, workspace_caps);
}

#[tokio::test]
async fn operator_cap_changes_and_overage_waivers_are_audited() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    let operator = UserId::new(Uuid::new_v4());
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(operator.as_uuid())
    .bind(format!("ai-usage-operator-{}", operator.as_uuid()))
    .bind(format!("ai-usage-{}@example.test", operator.as_uuid()))
    .execute(&pool)
    .await
    .expect("operator user exists for audit provenance");
    set_ai_usage_cap(
        &pool,
        AiUsageLimitScope::Workspace(workspace),
        10_000,
        operator,
    )
    .await
    .expect("workspace cap changes with an audit event");

    let ledger = PostgresUsageLedger::new(&pool);
    let reservation = ledger
        .reserve(request(project, Uuid::new_v4(), 100, 0, 1_000))
        .await
        .expect("reservation opens under the changed cap");
    let actual = UsageObservation {
        input_tokens: 150,
        output_tokens: 0,
        reasoning_tokens: 0,
        commercial_units: None,
    };
    ledger
        .observe(&reservation, actual.clone())
        .await
        .expect("overage evidence persists");
    ledger
        .settle(&reservation, actual, "test-price-v1")
        .await
        .expect("overage is settled and blocks the workspace");
    waive_ai_usage_overage(&pool, workspace, operator)
        .await
        .expect("operator waiver records the decision");

    let block = sqlx::query(
        "SELECT cleared_at, cleared_by_user_id, clear_reason
         FROM ai_usage_workspace_blocks WHERE workspace_id=$1",
    )
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("waived block remains in history");
    assert!(
        block
            .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cleared_at")
            .is_some()
    );
    assert_eq!(
        block.get::<Option<Uuid>, _>("cleared_by_user_id"),
        Some(operator.as_uuid())
    );
    assert_eq!(
        block.get::<Option<String>, _>("clear_reason").as_deref(),
        Some("operator_waiver")
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM admin_audit_events WHERE principal_user_id=$1
           AND event_type IN ('ai.usage_limit_changed', 'ai.usage_overage_waived')",
    )
    .bind(operator.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("operator actions are audited");
    assert_eq!(audit_count, 2);
}

#[tokio::test]
async fn n_minus_one_ledger_inserts_are_included_in_new_reservation_counters() {
    let Some(pool) = database().await else {
        return;
    };
    let (project, workspace) = fixture(&pool).await;
    sqlx::query(
        "UPDATE ai_usage_limits SET limit_micros = CASE scope_kind
           WHEN 'project' THEN 100 ELSE 100_000_000_000 END
         WHERE (scope_kind = 'platform' AND scope_id = $3)
            OR (scope_kind = 'workspace' AND scope_id = $2)
            OR (scope_kind = 'project' AND scope_id = $1)",
    )
    .bind(project.as_uuid())
    .bind(workspace.as_uuid())
    .bind(Uuid::from_u128(1))
    .execute(&pool)
    .await
    .expect("project cap is low while parent caps retain headroom");
    sqlx::query(
        "UPDATE ai_usage_period_buckets SET limit_micros = CASE scope_kind
           WHEN 'project' THEN 100 ELSE 100_000_000_000 END
         WHERE period_start = date_trunc('month', now() AT TIME ZONE 'UTC')::date
           AND ((scope_kind = 'platform' AND scope_id = $3)
             OR (scope_kind = 'workspace' AND scope_id = $2)
             OR (scope_kind = 'project' AND scope_id = $1))",
    )
    .bind(project.as_uuid())
    .bind(workspace.as_uuid())
    .bind(Uuid::from_u128(1))
    .execute(&pool)
    .await
    .expect("current hierarchy buckets follow the operator caps");
    sqlx::query(
        "INSERT INTO ai_usage_ledger
           (project_id, profile, provider, model, purpose, input_tokens, output_tokens, cost_micros)
         VALUES ($1, 'reasoning', 'legacy-provider', 'legacy-model', 'structured', 50, 0, 50)",
    )
    .bind(project.as_uuid())
    .execute(&pool)
    .await
    .expect("N-1 old ledger insert runs the new counter trigger");
    let ledger = PostgresUsageLedger::new(&pool);
    let reservation = ledger
        .reserve(request(project, Uuid::new_v4(), 60, 0, 100))
        .await;
    assert!(
        matches!(reservation, Err(AiError::UsageLimitExceeded)),
        "legacy spend did not deny the N reservation: {reservation:?}"
    );
    let period: chrono::NaiveDate =
        sqlx::query_scalar("SELECT date_trunc('month', now() AT TIME ZONE 'UTC')::date")
            .fetch_one(&pool)
            .await
            .expect("current UTC month");
    assert_eq!(
        bucket(&pool, "project", project.as_uuid(), period).await.1,
        50
    );
    assert_eq!(
        bucket(&pool, "workspace", workspace.as_uuid(), period)
            .await
            .1,
        50
    );
    assert_eq!(
        bucket(&pool, "platform", Uuid::from_u128(1), period)
            .await
            .1,
        50
    );
}
