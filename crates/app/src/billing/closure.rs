//! Provider-side closure of detached billing accounts (08c).
//!
//! Workspace deletion (directly or through account deletion, 11B) detaches the
//! account in the database and the `billing_track_provider_closure` trigger
//! records what is still owed at the provider: `cancel_pending` when a
//! subscription was captured, otherwise `customer_manual`. This module drives
//! that record forward. It never claims provider deletion: Dodo has no
//! customer-closure API, so the customer record always ends in an operator step
//! that is closed only with recorded evidence.
use super::*;
use deepref_application::inbox::InboxContext;

/// Detached accounts considered per reconciler pass.
const CLOSURE_BATCH: i64 = 16;

/// Webhook families that prove the captured subscription has ended.
const ENDED_EVENTS: &[&str] = &["subscription.cancelled", "subscription.expired"];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ClosurePass {
    pub cancelled: u64,
    pub manual_required: u64,
    pub retry_later: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClosureStatus {
    pub cancel_pending: i64,
    pub cancel_failed: i64,
    pub customer_manual: i64,
    pub closed: i64,
}

impl BillingService {
    /// Ask the provider to cancel subscriptions captured at detachment. No DB
    /// transaction is held across the call; the claim pushes `reconcile_after`
    /// forward so concurrent passes do not call twice for the same row, and the
    /// final update only applies while the row is still `cancel_pending`.
    pub async fn close_detached(&self) -> Result<ClosurePass, BillingStoreError> {
        let mut tx = transaction(&self.pool).await?;
        let due = sqlx::query(
            "UPDATE billing_accounts SET reconcile_after=now()+interval '15 minutes'
             WHERE id IN (
               SELECT id FROM billing_accounts
               WHERE provider_scope=$1 AND provisioning='detached'
                 AND provider_closure='cancel_pending' AND reconcile_after<=now()
               ORDER BY reconcile_after,id LIMIT $2 FOR UPDATE SKIP LOCKED)
             RETURNING id,closure_customer_ref,closure_subscription_ref",
        )
        .bind(self.provider.namespace())
        .bind(CLOSURE_BATCH)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut pass = ClosurePass::default();
        for row in due {
            let id: Uuid = row.try_get("id")?;
            let refs = row
                .try_get::<Option<String>, _>("closure_customer_ref")?
                .zip(row.try_get::<Option<String>, _>("closure_subscription_ref")?);
            let outcome = match refs {
                Some((customer, subscription)) => {
                    match (
                        CustomerReference::new(customer),
                        CustomerReference::new(subscription),
                    ) {
                        (Ok(customer), Ok(subscription)) => {
                            self.provider
                                .cancel_subscription(&customer, &subscription)
                                .await
                        }
                        _ => Ok(SubscriptionCancellation::ManualRequired),
                    }
                }
                // A subscription without a customer cannot be addressed safely.
                None => Ok(SubscriptionCancellation::ManualRequired),
            };
            let next = match outcome {
                Ok(SubscriptionCancellation::Cancelled) => {
                    pass.cancelled += 1;
                    "customer_manual"
                }
                Ok(SubscriptionCancellation::ManualRequired) => {
                    pass.manual_required += 1;
                    "cancel_failed"
                }
                Err(_) => {
                    pass.retry_later += 1;
                    continue;
                }
            };
            advance(&self.pool, id, "cancel_pending", next).await?;
        }
        Ok(pass)
    }

    /// Inbox effect for an event whose customer only matches a detached
    /// account. Completes the event (no projection exists to update). An ended
    /// subscription event for the captured subscription is closure evidence;
    /// any other event, duplicate or out-of-order delivery leaves state as is,
    /// because closure state only moves forward.
    pub(super) async fn detached_event(
        &self,
        customer: &str,
        payload: &serde_json::Value,
        context: &InboxContext,
    ) -> Result<bool, BillingStoreError> {
        let mut tx = transaction(&self.pool).await?;
        let row = sqlx::query(
            "SELECT id,closure_subscription_ref,provider_closure FROM billing_accounts
             WHERE provider_scope=$1 AND provisioning='detached' AND closure_customer_ref=$2
             ORDER BY detached_at DESC,id LIMIT 1 FOR UPDATE",
        )
        .bind(self.provider.namespace())
        .bind(customer)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        crate::inbox::fence_inbox_event_effect(&mut tx, context)
            .await
            .map_err(|_| BillingStoreError::Conflict)?;
        let subscription = payload
            .pointer("/data/subscription_id")
            .and_then(serde_json::Value::as_str);
        let captured: Option<String> = row.try_get("closure_subscription_ref")?;
        let state: Option<String> = row.try_get("provider_closure")?;
        if ENDED_EVENTS.contains(&context.event_type.as_str())
            && subscription.is_some()
            && subscription == captured.as_deref()
            && matches!(state.as_deref(), Some("cancel_pending" | "cancel_failed"))
        {
            sqlx::query(
                "UPDATE billing_accounts SET provider_closure='customer_manual',provider_closure_at=now() WHERE id=$1",
            )
            .bind(row.try_get::<Uuid, _>("id")?)
            .execute(&mut *tx)
            .await?;
        }
        crate::inbox::complete_inbox_event_in_transaction(&mut tx, context)
            .await
            .map_err(|_| BillingStoreError::Conflict)?;
        tx.commit().await?;
        Ok(true)
    }
}

async fn advance(pool: &PgPool, id: Uuid, from: &str, to: &str) -> Result<(), BillingStoreError> {
    let mut tx = transaction(pool).await?;
    sqlx::query(
        "UPDATE billing_accounts SET provider_closure=$3,provider_closure_at=now()
         WHERE id=$1 AND provider_closure=$2",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Operator step: record that provider-side closure (subscription ended and
/// the customer erasure request fulfilled by the provider) is evidenced by the
/// restricted record `evidence`. Only from `customer_manual` or
/// `cancel_failed`; returns false when the account is not in such a state.
pub async fn record_provider_closure(
    pool: &PgPool,
    account: Uuid,
    evidence: Uuid,
) -> Result<bool, BillingStoreError> {
    let mut tx = transaction(pool).await?;
    let updated = sqlx::query(
        "UPDATE billing_accounts SET provider_closure='closed',provider_closure_at=now(),provider_closure_evidence=$2
         WHERE id=$1 AND provisioning='detached' AND provider_closure IN ('cancel_failed','customer_manual')",
    )
    .bind(account)
    .bind(evidence)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(updated == 1)
}

/// Counts per closure state; no provider references or tenant identifiers.
pub async fn provider_closure_status(pool: &PgPool) -> Result<ClosureStatus, BillingStoreError> {
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE provider_closure='cancel_pending') AS cancel_pending,
                count(*) FILTER (WHERE provider_closure='cancel_failed') AS cancel_failed,
                count(*) FILTER (WHERE provider_closure='customer_manual') AS customer_manual,
                count(*) FILTER (WHERE provider_closure='closed') AS closed
         FROM billing_accounts WHERE provisioning='detached'",
    )
    .fetch_one(pool)
    .await?;
    Ok(ClosureStatus {
        cancel_pending: row.try_get("cancel_pending")?,
        cancel_failed: row.try_get("cancel_failed")?,
        customer_manual: row.try_get("customer_manual")?,
        closed: row.try_get("closed")?,
    })
}
