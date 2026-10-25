#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::panic_in_result_fn
)]

use chrono::{DateTime, Timelike, Utc};
use deepref_app::billing::{
    DodoUsageExportSink, UsageResolutionAction, UsageResolutionReason,
    list_unresolved_usage_exports, resolve_usage_export_hold,
};
use deepref_application::billing::{
    BillingFuture, BillingUsageProvider, DodoEventRecord, DodoIngestOutcome, DodoUsageEvent,
};
use deepref_application::usage_export::{AiUsageFact, UsageExportOutcome, UsageExportSink};
use deepref_domain::UserId;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 24).await
}

struct MockUsageProvider {
    business_id: String,
    namespace: String,
    ingest_result: Mutex<DodoIngestOutcome>,
    lookup_result: Mutex<Option<DodoEventRecord>>,
    received_events: Mutex<Vec<DodoUsageEvent>>,
}

impl MockUsageProvider {
    fn new(business_id: &str, namespace: &str) -> Self {
        Self {
            business_id: business_id.to_string(),
            namespace: namespace.to_string(),
            ingest_result: Mutex::new(DodoIngestOutcome::Accepted),
            lookup_result: Mutex::new(None),
            received_events: Mutex::new(Vec::new()),
        }
    }
}

impl BillingUsageProvider for MockUsageProvider {
    fn usage_namespace(&self) -> &str {
        &self.namespace
    }

    fn business_id(&self) -> &str {
        &self.business_id
    }

    fn ingest_usage_event<'a>(
        &'a self,
        event: DodoUsageEvent,
    ) -> BillingFuture<'a, DodoIngestOutcome> {
        Box::pin(async move {
            self.received_events.lock().unwrap().push(event);
            Ok(self.ingest_result.lock().unwrap().clone())
        })
    }

    fn lookup_usage_event<'a>(
        &'a self,
        _event_id: &'a str,
    ) -> BillingFuture<'a, Option<DodoEventRecord>> {
        Box::pin(async move { Ok(self.lookup_result.lock().unwrap().clone()) })
    }
}

fn sample_fact(
    workspace_id: Uuid,
    commercial_units: Option<i64>,
    correction_of: Option<Uuid>,
    settled_at: DateTime<Utc>,
) -> AiUsageFact {
    AiUsageFact {
        fact_id: Uuid::new_v4(),
        workspace_id,
        project_id: None,
        principal_kind: None,
        provider: "anthropic".into(),
        model: "claude-3".into(),
        input_tokens: Some(100),
        output_tokens: Some(50),
        reasoning_tokens: None,
        provider_cost_micros: 50_000,
        commercial_units,
        price_version: Some("v1".into()),
        period_start: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        settled_at,
        correction_of,
    }
}

async fn seed_billing_account(
    pool: &PgPool,
    workspace_id: Uuid,
    namespace: &str,
    customer_ref: &str,
) {
    let account_id = Uuid::new_v4();
    let op_id = Uuid::new_v4();
    let slug = format!("slug-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO workspaces (id, kind, name, slug)
         VALUES ($1, 'organization', 'test-ws', $2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(workspace_id)
    .bind(slug)
    .execute(pool)
    .await
    .expect("workspace seeds");

    sqlx::query(
        "INSERT INTO billing_accounts (id, workspace_id, provider_scope, operation_id, customer_ref, generation, provisioning)
         VALUES ($1, $2, $3, $4, $5, 1, 'mapped')
         ON CONFLICT (workspace_id, provider_scope) DO UPDATE SET customer_ref = $5, generation = billing_accounts.generation + 1",
    )
    .bind(account_id)
    .bind(workspace_id)
    .bind(namespace)
    .bind(op_id)
    .bind(customer_ref)
    .execute(pool)
    .await
    .expect("billing account seeds");
}

#[tokio::test]
async fn zero_invented_quantities_and_fail_closed_corrections() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink = DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();

    // 1. None commercial_units -> zero invented quantity! Fail closed permanently.
    let fact_no_units = sample_fact(workspace_id, None, None, Utc::now());
    let outcome = sink.deliver(&fact_no_units).await;
    match outcome {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(reason.contains("unconfigured commercial units"));
        }
        other => panic!("expected PermanentFailure for None commercial_units, got {other:?}"),
    }

    // 2. Non-positive commercial units (e.g. 0) -> fail closed permanently.
    let fact_zero_units = sample_fact(workspace_id, Some(0), None, Utc::now());
    let outcome = sink.deliver(&fact_zero_units).await;
    match outcome {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(reason.contains("non-positive commercial units"));
        }
        other => panic!("expected PermanentFailure for 0 commercial_units, got {other:?}"),
    }

    // 3. Correction fact -> fail closed permanently (requires manual reconciliation).
    let fact_correction = sample_fact(workspace_id, Some(10), Some(Uuid::new_v4()), Utc::now());
    let outcome = sink.deliver(&fact_correction).await;
    match outcome {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(reason.contains("not proven by verified contract"));
        }
        other => panic!("expected PermanentFailure for correction, got {other:?}"),
    }
}

#[tokio::test]
async fn disabled_meter_returns_retryable_failure_preserving_facts() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    // disabled meter
    let sink = DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        false,
        "ai_commercial_units".into(),
    );

    let fact = sample_fact(Uuid::new_v4(), Some(10), None, Utc::now());
    let outcome = sink.deliver(&fact).await;
    match outcome {
        UsageExportOutcome::RetryableFailure(reason) => {
            assert!(reason.contains("disabled in runtime configuration"));
        }
        other => panic!("expected RetryableFailure for disabled meter, got {other:?}"),
    }
}

async fn seed_outbox_fact(pool: &PgPool, fact: &AiUsageFact) {
    sqlx::query(
        "INSERT INTO outbox_events (event_id, message_type, message_version, payload, expected_consumers, delivery_state)
         VALUES ($1, 'ai_usage.fact', 1, jsonb_build_object('fact_id', $1, 'workspace_id', $2, 'settled_at', $3::text), ARRAY['ai_usage_export'], 'pending')
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(fact.fact_id)
    .bind(fact.workspace_id)
    .bind(fact.settled_at.to_rfc3339())
    .execute(pool)
    .await
    .expect("seed outbox fact");
}

#[tokio::test]
async fn late_fact_lookup_before_refusal() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink = DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();
    seed_billing_account(&pool, workspace_id, "dodo_test", "cus_late_test").await;

    // Fact settled 2 hours ago (> 50 minutes past)
    let late_settled = Utc::now() - chrono::Duration::hours(2);
    let fact = sample_fact(workspace_id, Some(5), None, late_settled);
    seed_outbox_fact(&pool, &fact).await;

    // Case A: provider lookup returns 404 (None) -> fails permanently due to acceptance window
    *mock.lookup_result.lock().unwrap() = None;
    let outcome = sink.deliver(&fact).await;
    match outcome {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(reason.contains("outside provider acceptance window"));
        }
        other => panic!("expected PermanentFailure for aged fact not on provider, got {other:?}"),
    }

    // Case B: provider lookup returns existing record with matching canonical wire payload
    let mut meta = std::collections::BTreeMap::new();
    meta.insert("amount".into(), serde_json::Value::from(5));
    meta.insert("model".into(), serde_json::Value::from("claude-3"));
    meta.insert("provider".into(), serde_json::Value::from("anthropic"));
    *mock.lookup_result.lock().unwrap() = Some(DodoEventRecord {
        business_id: "bus_test".into(),
        customer_id: "cus_late_test".into(),
        event_id: fact.fact_id.to_string(),
        event_name: "ai_commercial_units".into(),
        timestamp: late_settled.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        metadata: Some(meta),
    });

    let outcome2 = sink.deliver(&fact).await;
    assert_eq!(outcome2, UsageExportOutcome::Duplicate);
}

#[tokio::test]
async fn durable_target_binding_prevents_customer_redirection() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink = DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();
    seed_billing_account(&pool, workspace_id, "dodo_test", "cus_initial").await;

    let fact = sample_fact(workspace_id, Some(10), None, Utc::now());
    seed_outbox_fact(&pool, &fact).await;

    // First deliver attempt successfully freezes customer target
    let outcome1 = sink.deliver(&fact).await;
    assert_eq!(outcome1, UsageExportOutcome::Accepted);

    // Verify frozen mapping in billing_usage_fact_targets
    let frozen_cus: String = sqlx::query_scalar(
        "SELECT customer_ref FROM billing_usage_fact_targets WHERE fact_id = $1",
    )
    .bind(fact.fact_id)
    .fetch_one(&pool)
    .await
    .expect("frozen row exists");
    assert_eq!(frozen_cus, "cus_initial");

    // Later workspace billing account is detached or remapped to cus_new
    sqlx::query(
        "UPDATE billing_accounts SET customer_ref = 'cus_new', generation = generation + 1 WHERE workspace_id = $1 AND provider_scope = 'dodo_test'",
    )
    .bind(workspace_id)
    .execute(&pool)
    .await
    .unwrap();

    // Subsequent delivery of same fact detects mismatch and fails permanently
    let outcome2 = sink.deliver(&fact).await;
    match outcome2 {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(reason.contains("historical fact target customer does not match"));
        }
        other => panic!("expected PermanentFailure when customer remapped, got {other:?}"),
    }
}

#[tokio::test]
async fn operator_resolution_workflow() {
    let Some(pool) = database().await else { return };

    let workspace_id = Uuid::new_v4();
    let fact_id = Uuid::new_v4();
    let operator_user_id = UserId::new(Uuid::new_v4());
    let op_workos_id = format!("user_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, $2, 'operator@test.local', 'active')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(operator_user_id.as_uuid())
    .bind(op_workos_id)
    .execute(&pool)
    .await
    .unwrap();

    let slug = format!("slug-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO workspaces (id, kind, name, slug)
         VALUES ($1, 'organization', 'test-ws', $2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(workspace_id)
    .bind(slug)
    .execute(&pool)
    .await
    .unwrap();

    // Seed outbox event & receipt in quarantined state
    sqlx::query(
        "INSERT INTO outbox_events (event_id, message_type, message_version, payload, expected_consumers, delivery_state)
         VALUES ($1, 'ai_usage.fact', 1, jsonb_build_object('fact_id', $1, 'workspace_id', $2, 'settled_at', now()::text), ARRAY['ai_usage_export'], 'quarantined')",
    )
    .bind(fact_id)
    .bind(workspace_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO outbox_receipts (event_id, consumer, state, attempts, next_attempt_at)
         VALUES ($1, 'ai_usage_export', 'quarantined', 5, now())",
    )
    .bind(fact_id)
    .execute(&pool)
    .await
    .unwrap();

    // List unresolved exports
    let unresolved = list_unresolved_usage_exports(&pool, 10).await.unwrap();
    assert!(unresolved.iter().any(|u| u.fact_id == fact_id));

    // Operator resolves hold
    let resolution_id = Uuid::new_v4();
    resolve_usage_export_hold(
        &pool,
        fact_id,
        operator_user_id,
        resolution_id,
        UsageResolutionAction::ManualReconciliation,
        UsageResolutionReason::ManualCreditIssued,
        Some("operator approved billing adjustment after invoice review".into()),
    )
    .await
    .unwrap();

    // Verify outbox receipt remains quarantined (NOT faked as delivered)
    let state: String = sqlx::query_scalar(
        "SELECT state FROM outbox_receipts WHERE consumer = 'ai_usage_export' AND event_id = $1",
    )
    .bind(fact_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "quarantined");

    // Verify audit row in admin_audit_events conforms to 0168 schema
    let audit_event: (String, serde_json::Value) = sqlx::query_as(
        "SELECT event_type, metadata FROM admin_audit_events WHERE workspace_id = $1 ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(audit_event.0, "billing.changed");
    assert_eq!(audit_event.1["reason"], "review_resolved");
    assert_eq!(audit_event.1["operation_id"], resolution_id.to_string());
}

#[tokio::test]
async fn regression_ambiguous_then_404_two_deliver_calls_single_post() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    // First call returns Ambiguous, and subsequent lookup returns 404 (None)
    *mock.ingest_result.lock().unwrap() = DodoIngestOutcome::Ambiguous;
    *mock.lookup_result.lock().unwrap() = None;

    let sink = DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();
    seed_billing_account(&pool, workspace_id, "dodo_test", "cus_ambig_test").await;

    // Fact settled 5 minutes ago (well within acceptance window)
    let fact = sample_fact(
        workspace_id,
        Some(10),
        None,
        Utc::now() - chrono::Duration::minutes(5),
    );
    seed_outbox_fact(&pool, &fact).await;

    // Call 1: ambiguous POST, lookup returns 404 -> outcome is Ambiguous
    let outcome1 = sink.deliver(&fact).await;
    match outcome1 {
        UsageExportOutcome::Ambiguous(_) => {}
        other => panic!("expected Ambiguous outcome on first call, got {other:?}"),
    }
    let post_count_after_call1 = mock.received_events.lock().unwrap().len();
    assert_eq!(
        post_count_after_call1, 1,
        "first deliver call must execute exactly 1 POST"
    );

    // Call 2: retry of same fact within acceptance window
    // Contract requirement: once dispatch intent was committed, deliver must NOT re-POST
    // blindly if lookup returns 404; it must remain lookup-only and produce ONE POST total.
    let outcome2 = sink.deliver(&fact).await;
    match outcome2 {
        UsageExportOutcome::Ambiguous(_) => {}
        other => panic!("expected Ambiguous outcome on second call, got {other:?}"),
    }
    let total_post_count = mock.received_events.lock().unwrap().len();
    assert_eq!(
        total_post_count, 1,
        "ambiguous followed by 404 over two deliver calls must produce exactly ONE POST total"
    );
}

#[tokio::test]
async fn regression_concurrent_calls_single_post() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink1 = Arc::new(DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        true,
        "ai_commercial_units".into(),
    ));
    let sink2 = Arc::clone(&sink1);

    let workspace_id = Uuid::new_v4();
    seed_billing_account(&pool, workspace_id, "dodo_test", "cus_concurrent_test").await;

    let fact = sample_fact(
        workspace_id,
        Some(15),
        None,
        Utc::now() - chrono::Duration::minutes(2),
    );
    seed_outbox_fact(&pool, &fact).await;

    let fact_clone = fact.clone();
    let task1 = tokio::spawn(async move { sink1.deliver(&fact).await });
    let task2 = tokio::spawn(async move { sink2.deliver(&fact_clone).await });

    let (res1, res2) = tokio::join!(task1, task2);
    let out1 = res1.unwrap();
    let out2 = res2.unwrap();

    // Both tasks complete safely: one wins exclusive dispatch claim (Accepted),
    // the concurrent worker sees claimed dispatch and performs lookup-only.
    assert!(
        matches!(
            out1,
            UsageExportOutcome::Accepted
                | UsageExportOutcome::Duplicate
                | UsageExportOutcome::Ambiguous(_)
        ),
        "task 1 should complete safely: {out1:?}"
    );
    assert!(
        matches!(
            out2,
            UsageExportOutcome::Accepted
                | UsageExportOutcome::Duplicate
                | UsageExportOutcome::Ambiguous(_)
        ),
        "task 2 should complete safely: {out2:?}"
    );

    let total_post_count = mock.received_events.lock().unwrap().len();
    assert_eq!(
        total_post_count, 1,
        "concurrent deliver calls must transactionally claim dispatch right and produce exactly ONE POST total"
    );
}

#[tokio::test]
async fn regression_namespace_eventname_drift_immutable_wire() {
    let Some(pool) = database().await else { return };

    let mock1 = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink1 = DodoUsageExportSink::new(
        pool.clone(),
        mock1.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();
    seed_billing_account(&pool, workspace_id, "dodo_test", "cus_drift_test").await;

    let fact = sample_fact(workspace_id, Some(20), None, Utc::now());
    seed_outbox_fact(&pool, &fact).await;

    // Delivery 1 binds and freezes canonical wire (event_name = "ai_commercial_units")
    let out1 = sink1.deliver(&fact).await;
    assert_eq!(out1, UsageExportOutcome::Accepted);

    // Now configure Sink 2 with drifted event_name "ai_commercial_units_v2"
    let mock2 = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink2 = DodoUsageExportSink::new(
        pool.clone(),
        mock2.clone(),
        true,
        "ai_commercial_units_v2".into(),
    );

    // Delivery 2 for same fact with drifted configuration MUST fail permanently
    let out2 = sink2.deliver(&fact).await;
    match out2 {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(
                reason.contains("drift") || reason.contains("canonical"),
                "expected permanent refusal on canonical wire drift, got: {reason}"
            );
        }
        other => panic!("expected PermanentFailure on drifted wire configuration, got {other:?}"),
    }
}

#[tokio::test]
async fn regression_operator_reopen_cannot_authorize_stale_preresolution_receipt_and_preserves_other_recipients()
 {
    let Some(pool) = database().await else { return };

    let workspace_id = Uuid::new_v4();
    let fact_id = Uuid::new_v4();
    let operator_user_id = UserId::new(Uuid::new_v4());
    let op_workos_id = format!("user_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, $2, 'operator2@test.local', 'active')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(operator_user_id.as_uuid())
    .bind(op_workos_id)
    .execute(&pool)
    .await
    .unwrap();

    let slug = format!("slug-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO workspaces (id, kind, name, slug)
         VALUES ($1, 'organization', 'test-ws-2', $2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(workspace_id)
    .bind(slug)
    .execute(&pool)
    .await
    .unwrap();

    // Outbox event with claim_generation = 5, quarantined
    sqlx::query(
        "INSERT INTO outbox_events (event_id, message_type, message_version, payload, expected_consumers, delivery_state, claim_generation)
         VALUES ($1, 'ai_usage.fact', 1, jsonb_build_object('fact_id', $1, 'workspace_id', $2, 'settled_at', now()::text), ARRAY['ai_usage_export', 'audit_consumer'], 'quarantined', 5)",
    )
    .bind(fact_id)
    .bind(workspace_id)
    .execute(&pool)
    .await
    .unwrap();

    // Consumer 1: ai_usage_export is quarantined at generation 5
    sqlx::query(
        "INSERT INTO outbox_receipts (event_id, consumer, state, attempts, attempt_generation, settled_generation, next_attempt_at)
         VALUES ($1, 'ai_usage_export', 'quarantined', 5, 5, 5, now())",
    )
    .bind(fact_id)
    .execute(&pool)
    .await
    .unwrap();

    // Consumer 2: audit_consumer is delivered at generation 4
    sqlx::query(
        "INSERT INTO outbox_receipts (event_id, consumer, state, attempts, attempt_generation, settled_generation, delivered_at)
         VALUES ($1, 'audit_consumer', 'delivered', 1, 4, 4, now())",
    )
    .bind(fact_id)
    .execute(&pool)
    .await
    .unwrap();

    // Operator reopens the hold via ReopenedRetry
    let resolution_id = Uuid::new_v4();
    resolve_usage_export_hold(
        &pool,
        fact_id,
        operator_user_id,
        resolution_id,
        UsageResolutionAction::ReopenedRetry,
        UsageResolutionReason::ExternalAdjustmentConfirmed,
        Some("operator verified external adjustment, allowing fresh retry".into()),
    )
    .await
    .unwrap();

    // 1. Audit consumer must remain delivered
    let audit_state: (String, i64) = sqlx::query_as(
        "SELECT state, settled_generation FROM outbox_receipts WHERE event_id = $1 AND consumer = 'audit_consumer'",
    )
    .bind(fact_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_state.0, "delivered",
        "other consumer must remain untouched"
    );
    assert_eq!(
        audit_state.1, 4,
        "other consumer settled generation must be preserved"
    );

    // 2. Monotonic fence invariant: outbox_events claim_generation must advance (> 5)
    let event_claim_gen: i64 =
        sqlx::query_scalar("SELECT claim_generation FROM outbox_events WHERE event_id = $1")
            .bind(fact_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        event_claim_gen > 5,
        "event claim_generation must advance monotonically on reopen (got {event_claim_gen}, expected > 5)"
    );

    // 3. Receipt for ai_usage_export must be retrying, attempts reset to 0, but attempt_generation/settled_generation NEVER reset to 0
    let receipt: (String, i32, i64, i64) = sqlx::query_as(
        "SELECT state, attempts, attempt_generation, settled_generation FROM outbox_receipts WHERE event_id = $1 AND consumer = 'ai_usage_export'",
    )
    .bind(fact_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt.0, "retrying");
    assert_eq!(
        receipt.1, 0,
        "authorized bounded attempt counters must be reset"
    );
    assert!(
        receipt.2 >= 5 && receipt.3 >= 5,
        "monotonic fences must NEVER be reset to 0; attempt_generation={} settled_generation={}",
        receipt.2,
        receipt.3
    );
}

#[tokio::test]
async fn regression_fractional_timestamp_preserved_without_truncation() {
    let Some(pool) = database().await else { return };

    let mock = Arc::new(MockUsageProvider::new("bus_test", "dodo_test"));
    let sink = DodoUsageExportSink::new(
        pool.clone(),
        mock.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();
    seed_billing_account(&pool, workspace_id, "dodo_test", "cus_frac_ts").await;

    // Settle within acceptance window (-50m, +4m) with non-zero fractional seconds (microseconds)
    let settled_at = (Utc::now() - chrono::Duration::minutes(1))
        .with_nanosecond(654_321_000)
        .unwrap();

    let fact = sample_fact(workspace_id, Some(10), None, settled_at);
    seed_outbox_fact(&pool, &fact).await;

    let outcome = sink.deliver(&fact).await;
    assert_eq!(outcome, UsageExportOutcome::Accepted);

    // 1. Verify provider wire event timestamp preserved fractional seconds without truncation
    {
        let events = mock.received_events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].timestamp.contains(".654321"),
            "expected wire timestamp to preserve fractional seconds, got: {}",
            events[0].timestamp
        );
    }

    // 2. Verify frozen row in billing_usage_fact_targets preserves exact original timestamp and digest
    let (frozen_ts, frozen_digest): (chrono::DateTime<chrono::Utc>, Option<String>) = sqlx::query_as(
        "SELECT original_timestamp, wire_digest FROM billing_usage_fact_targets WHERE fact_id = $1",
    )
    .bind(fact.fact_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(frozen_ts.timestamp_micros(), settled_at.timestamp_micros());
    assert!(frozen_digest.is_some());
}

#[tokio::test]
async fn regression_target_immutable_scope_drift_rejected_even_with_same_customer() {
    let Some(pool) = database().await else { return };

    let mock1 = Arc::new(MockUsageProvider::new("bus_test", "dodo_scope_a"));
    let sink1 = DodoUsageExportSink::new(
        pool.clone(),
        mock1.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let mock2 = Arc::new(MockUsageProvider::new("bus_test", "dodo_scope_b"));
    let sink2 = DodoUsageExportSink::new(
        pool.clone(),
        mock2.clone(),
        true,
        "ai_commercial_units".into(),
    );

    let workspace_id = Uuid::new_v4();
    // Both scopes share the same customer reference
    seed_billing_account(&pool, workspace_id, "dodo_scope_a", "cus_shared_customer").await;
    seed_billing_account(&pool, workspace_id, "dodo_scope_b", "cus_shared_customer").await;

    let fact = sample_fact(workspace_id, Some(20), None, Utc::now());
    seed_outbox_fact(&pool, &fact).await;

    // First deliver attempt successfully binds fact to scope A
    let outcome1 = sink1.deliver(&fact).await;
    assert_eq!(outcome1, UsageExportOutcome::Accepted);

    // Second deliver attempt under scope B must reject the fact even though same customer
    let outcome2 = sink2.deliver(&fact).await;
    match outcome2 {
        UsageExportOutcome::PermanentFailure(reason) => {
            assert!(
                reason.contains("provider scope drifted"),
                "expected provider scope drift rejection, got: {reason}"
            );
        }
        other => panic!("expected PermanentFailure for drifted provider scope, got {other:?}"),
    }
}
