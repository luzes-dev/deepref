#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Focused real-PostgreSQL billing boundaries and provider-effect races.
use chrono::Utc;
use deepref_app::billing::{self, BillingService, BillingStoreError};
use deepref_application::billing::*;
use deepref_domain::{UserId, WorkspaceId, billing::*};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

struct Gate {
    block: AtomicBool,
    entered: Semaphore,
    release: Notify,
}
impl Gate {
    fn new() -> Self {
        Self {
            block: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Notify::new(),
        }
    }
    async fn wait(&self) {
        if self.block.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.notified().await;
        }
    }
    async fn started(&self) {
        self.entered.acquire().await.unwrap().forget();
    }
}
struct Fake {
    create_calls: AtomicUsize,
    ambiguous: AtomicBool,
    resolve: AtomicBool,
    active: AtomicBool,
    gate: Gate,
    create_gate: Gate,
    link_gate: Gate,
}
impl Fake {
    fn new() -> Self {
        Self {
            create_calls: AtomicUsize::new(0),
            ambiguous: AtomicBool::new(false),
            resolve: AtomicBool::new(false),
            active: AtomicBool::new(true),
            gate: Gate::new(),
            create_gate: Gate::new(),
            link_gate: Gate::new(),
        }
    }
}
impl BillingProvider for Fake {
    fn namespace(&self) -> &str {
        "dodo:test_mode:bus_fixture"
    }
    fn create_customer<'a>(
        &'a self,
        _: CustomerIntent<'a>,
    ) -> BillingFuture<'a, CustomerReference> {
        Box::pin(async move {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            self.create_gate.wait().await;
            if self.ambiguous.load(Ordering::SeqCst) {
                Err(BillingError::Ambiguous)
            } else {
                CustomerReference::new("cus_fixture".into())
            }
        })
    }
    fn resolve_customer<'a>(&'a self, _: Uuid) -> BillingFuture<'a, CustomerResolution> {
        Box::pin(async move {
            Ok(if self.resolve.load(Ordering::SeqCst) {
                CustomerResolution::Found(CustomerReference::new("cus_fixture".into())?)
            } else {
                CustomerResolution::Unresolved
            })
        })
    }
    fn checkout<'a>(
        &'a self,
        _: &'a CustomerReference,
        _: Offering,
        _: &'a str,
    ) -> BillingFuture<'a, HostedLink> {
        Box::pin(async move {
            self.link_gate.wait().await;
            Ok(HostedLink::validated(
                "https://test.checkout.dodopayments.com/session/fixture".into(),
            ))
        })
    }
    fn portal<'a>(&'a self, c: &'a CustomerReference, p: &'a str) -> BillingFuture<'a, HostedLink> {
        self.checkout(c, Offering::Primary, p)
    }
    fn observe<'a>(
        &'a self,
        _: &'a CustomerReference,
    ) -> BillingFuture<'a, SubscriptionObservation> {
        Box::pin(async move {
            let active = self.active.load(Ordering::SeqCst);
            let at = Utc::now();
            self.gate.wait().await;
            Ok(SubscriptionObservation {
                reference: Some(SubscriptionReference::new(
                    "sub_fixture".into(),
                    "pdt_fixture".into(),
                )?),
                state: if active {
                    SubscriptionState::Active
                } else {
                    SubscriptionState::Cancelled
                },
                entitlements: Entitlements {
                    features: vec![Feature::AiFirst],
                    projects: 4,
                    members: 2,
                    monthly_ai_units: 0,
                },
                observed_at: at,
                review: None,
            })
        })
    }
    fn verify(
        &self,
        body: &[u8],
        headers: WebhookHeaders<'_>,
        _: chrono::DateTime<Utc>,
    ) -> Result<VerifiedBillingWebhook, BillingError> {
        if headers.signatures != "fixture-authenticated" {
            return Err(BillingError::Refused);
        }
        let payload: Value = serde_json::from_slice(body).map_err(|_| BillingError::Contract)?;
        let event_type = payload["type"].as_str().unwrap().to_string();
        Ok(VerifiedBillingWebhook {
            identity: headers.id.into(),
            customer: Some(CustomerReference::new("cus_fixture".into())?),
            adverse: event_type.starts_with("dispute.") || event_type.starts_with("credit."),
            event_type,
            payload,
        })
    }
}
async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
}
async fn seed(pool: &PgPool) -> (UserId, WorkspaceId) {
    let u = UserId::new(Uuid::new_v4());
    let w = WorkspaceId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,$2,'billing@example.test','active')").bind(u.as_uuid()).bind(format!("billing-{}",u.as_uuid())).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','billing fixture',$2)",
    )
    .bind(w.as_uuid())
    .bind(format!("billing-{}", w.as_uuid()))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')").bind(Uuid::new_v4()).bind(w.as_uuid()).bind(u.as_uuid()).execute(pool).await.unwrap();
    (u, w)
}
fn contact() -> BillingContact {
    BillingContact::parse("billing@example.test".into(), "Owner".into()).unwrap()
}
async fn setup() -> Option<(PgPool, UserId, WorkspaceId, Arc<Fake>, BillingService)> {
    let pool = database().await?;
    let (u, w) = seed(&pool).await;
    let fake = Arc::new(Fake::new());
    let service = BillingService::new(pool.clone(), fake.clone());
    Some((pool, u, w, fake, service))
}
async fn provision(service: &BillingService, u: UserId, w: WorkspaceId) {
    service.provision(u, w, contact()).await.unwrap();
}
async fn projection(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT projection FROM billing_accounts")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn concurrent_provisioning_uses_one_post_and_ambiguity_never_recreates() {
    let Some((pool, u, w, fake, service)) = setup().await else {
        return;
    };
    assert_eq!(
        service.read(u, w).await.unwrap().effective.classification,
        BillingClassification::Unknown
    );
    fake.create_gate.block.store(true, Ordering::SeqCst);
    fake.ambiguous.store(true, Ordering::SeqCst);
    let s = service.clone();
    let task = tokio::spawn(async move { s.provision(u, w, contact()).await });
    fake.create_gate.started().await;
    assert!(matches!(
        service.provision(u, w, contact()).await,
        Err(BillingStoreError::Conflict)
    ));
    fake.create_gate.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(BillingStoreError::Provider(BillingError::Ambiguous))
    ));
    assert!(service.reconcile_workspace(w).await.is_err());
    assert!(service.read(u, w).await.unwrap().provisioning_ambiguous);
    assert_eq!(fake.create_calls.load(Ordering::SeqCst), 1);
    fake.resolve.store(true, Ordering::SeqCst);
    service.reconcile_workspace(w).await.unwrap();
    service.reconcile_workspace(w).await.unwrap();
    assert_eq!(
        service.read(u, w).await.unwrap().effective.classification,
        BillingClassification::Paid
    );
    let references = sqlx::query("SELECT subscription_ref,product_ref FROM billing_accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        references.get::<String, _>("subscription_ref"),
        "sub_fixture"
    );
    assert_eq!(references.get::<String, _>("product_ref"), "pdt_fixture");
    let public = serde_json::to_string(&service.read(u, w).await.unwrap()).unwrap();
    assert!(
        !public.contains("sub_fixture")
            && !public.contains("pdt_fixture")
            && !public.contains("cus_fixture")
    );
    assert!(
        billing::enroll_free(&pool, w, Entitlements::default())
            .await
            .is_err()
    );
    assert_eq!(fake.create_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn stale_active_fetch_cannot_overwrite_newer_cancellation() {
    let Some((pool, u, w, fake, service)) = setup().await else {
        return;
    };
    provision(&service, u, w).await;
    fake.gate.block.store(true, Ordering::SeqCst);
    let s = service.clone();
    let task = tokio::spawn(async move { s.reconcile_workspace(w).await });
    fake.gate.started().await;
    fake.active.store(false, Ordering::SeqCst);
    service.reconcile_workspace(w).await.unwrap();
    fake.gate.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(BillingStoreError::Conflict)
    ));
    assert_eq!(projection(&pool).await["state"], "cancelled");
    assert!(
        service
            .read(u, w)
            .await
            .unwrap()
            .effective
            .entitlements
            .features
            .is_empty()
    );
}
#[tokio::test]
async fn adverse_event_fences_stale_activation_and_active_reads_never_extend_grace() {
    let Some((pool, u, w, fake, service)) = setup().await else {
        return;
    };
    provision(&service, u, w).await;
    service.reconcile_workspace(w).await.unwrap();
    let before = projection(&pool).await;
    fake.gate.block.store(true, Ordering::SeqCst);
    let s = service.clone();
    let task = tokio::spawn(async move { s.reconcile_workspace(w).await });
    fake.gate.started().await;
    let body = br#"{"type":"dispute.opened","data":{"customer":{"customer_id":"cus_fixture"}}}"#;
    let headers = || WebhookHeaders {
        id: "evt_fixture",
        timestamp: "0",
        signatures: "fixture-authenticated",
    };
    assert!(matches!(
        billing::receive(&service, body, headers()).await.unwrap(),
        deepref_app::inbox::ReceiveOutcome::Accepted(_)
    ));
    fake.gate.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(BillingStoreError::Conflict)
    ));
    service.reconcile_workspace(w).await.unwrap();
    assert_eq!(projection(&pool).await, before);
    assert!(service.read(u, w).await.unwrap().review_required);
    let generation: i64 = sqlx::query_scalar("SELECT generation FROM billing_accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(matches!(
        billing::receive(&service, body, headers()).await.unwrap(),
        deepref_app::inbox::ReceiveOutcome::Duplicate
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT generation FROM billing_accounts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        generation
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM integration_inbox_events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let bad = WebhookHeaders {
        id: "evt_bad",
        timestamp: "0",
        signatures: "invalid",
    };
    assert!(billing::receive(&service, body, bad).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM integration_inbox_events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}
#[tokio::test]
async fn link_is_withheld_when_membership_is_revoked_during_http() {
    let Some((pool, u, w, fake, service)) = setup().await else {
        return;
    };
    provision(&service, u, w).await;
    fake.link_gate.block.store(true, Ordering::SeqCst);
    let s = service.clone();
    let task = tokio::spawn(async move { s.checkout(u, w, Offering::Primary, "/settings").await });
    fake.link_gate.started().await;
    sqlx::query("DELETE FROM workspace_memberships WHERE workspace_id=$1")
        .bind(w.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    fake.link_gate.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(BillingStoreError::Denied)
    ));
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM admin_audit_events WHERE event_type='billing.changed' AND metadata->>'reason'='checkout_created'").fetch_one(&pool).await.unwrap(),0);
}
#[tokio::test]
async fn deletion_fences_inflight_response_and_detaches_provider_identity() {
    let Some((pool, u, w, fake, service)) = setup().await else {
        return;
    };
    fake.create_gate.block.store(true, Ordering::SeqCst);
    let s = service.clone();
    let task = tokio::spawn(async move { s.provision(u, w, contact()).await });
    fake.create_gate.started().await;
    sqlx::query("DELETE FROM workspace_memberships WHERE workspace_id=$1")
        .bind(w.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id=$1")
        .bind(w.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    fake.create_gate.release.notify_one();
    assert!(task.await.unwrap().is_err());
    let row = sqlx::query(
        "SELECT workspace_id,customer_ref,provisioning,purge_after FROM billing_accounts",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.get::<Option<Uuid>, _>("workspace_id").is_none());
    assert!(row.get::<Option<String>, _>("customer_ref").is_none());
    assert_eq!(row.get::<String, _>("provisioning"), "detached");
    assert!(
        row.get::<Option<chrono::DateTime<Utc>>, _>("purge_after")
            .is_some()
    );
}
#[tokio::test]
async fn legitimate_free_enrollment_is_explicit_and_provider_activation_removes_it() {
    let Some((pool, u, w, _, service)) = setup().await else {
        return;
    };
    billing::enroll_free(&pool, w, Entitlements::default())
        .await
        .unwrap();
    assert_eq!(
        service.read(u, w).await.unwrap().effective.classification,
        BillingClassification::KnownFree
    );
    provision(&service, u, w).await;
    assert_eq!(
        service.read(u, w).await.unwrap().effective.classification,
        BillingClassification::Unknown
    );
}

#[tokio::test]
async fn adverse_receipt_before_customer_mapping_is_held_when_replayed() {
    let Some((pool, u, w, fake, service)) = setup().await else {
        return;
    };
    fake.ambiguous.store(true, Ordering::SeqCst);
    assert!(service.provision(u, w, contact()).await.is_err());
    let body = br#"{"type":"credit.deducted","data":{"customer_id":"cus_fixture"}}"#;
    billing::receive(
        &service,
        body,
        WebhookHeaders {
            id: "evt_before_mapping",
            timestamp: "0",
            signatures: "fixture-authenticated",
        },
    )
    .await
    .unwrap();
    fake.resolve.store(true, Ordering::SeqCst);
    service.reconcile_workspace(w).await.unwrap();
    let registry = billing::registry(service.clone()).unwrap();
    assert_eq!(
        deepref_app::inbox::process_inbox_batch(
            &pool,
            &registry,
            "billing-test",
            std::time::Duration::from_secs(30),
            1
        )
        .await
        .unwrap(),
        1
    );
    assert!(
        service.read(u, w).await.unwrap().review_required,
        "an event received before mapping must not lose its adverse hold"
    );
    sqlx::query("UPDATE integration_inbox_events SET payload_expires_at=now()-interval '1 second' WHERE processing_state='processed'").execute(&pool).await.unwrap();
    assert_eq!(
        deepref_app::inbox::purge_processed_inbox_payloads(&pool, 1)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM billing_inbox_invalidations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM integration_inbox_events WHERE payload IS NULL"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn receipt_commit_failure_rolls_back_invalidation_and_audit_shape_is_closed() {
    let Some((pool, u, w, _, service)) = setup().await else {
        return;
    };
    provision(&service, u, w).await;
    let generation: i64 = sqlx::query_scalar("SELECT generation FROM billing_accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE integration_inbox_events ADD CONSTRAINT fixture_reject_event CHECK (event_type <> 'credit.deducted') NOT VALID").execute(&pool).await.unwrap();
    let result = billing::receive(
        &service,
        br#"{"type":"credit.deducted","data":{"customer_id":"cus_fixture"}}"#,
        WebhookHeaders {
            id: "evt_reject",
            timestamp: "0",
            signatures: "fixture-authenticated",
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM integration_inbox_events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT generation FROM billing_accounts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        generation
    );
    for metadata in [
        serde_json::json!({"operation_id":Uuid::new_v4(),"reason":null}),
        serde_json::json!({"operation_id":Uuid::new_v4(),"reason":"projection_updated","email":"private@example.test"}),
    ] {
        assert!(sqlx::query("INSERT INTO admin_audit_events(id,event_type,principal_kind,workspace_id,metadata) VALUES($1,'billing.changed','system',$2,$3)").bind(Uuid::new_v4()).bind(w.as_uuid()).bind(metadata).execute(&pool).await.is_err());
    }
}
