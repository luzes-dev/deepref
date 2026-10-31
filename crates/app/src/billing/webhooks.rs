use super::*;
use crate::inbox::ReceiveOutcome;
use deepref_application::inbox::{
    InboxHandlerError, InboxRegistry, InboxRegistryError, TypedInboxHandler,
};
use serde_json::Value;

pub const EVENT_TYPES: &[&str] = &[
    "subscription.active",
    "subscription.renewed",
    "subscription.on_hold",
    "subscription.past_due",
    "subscription.paused",
    "subscription.unpaused",
    "subscription.cancelled",
    "subscription.failed",
    "subscription.expired",
    "subscription.plan_changed",
    "subscription.updated",
    "subscription.update_payment_method",
    "payment.succeeded",
    "payment.failed",
    "payment.processing",
    "payment.cancelled",
    "refund.succeeded",
    "refund.failed",
    "dispute.opened",
    "dispute.expired",
    "dispute.accepted",
    "dispute.cancelled",
    "dispute.challenged",
    "dispute.won",
    "dispute.lost",
    "credit.added",
    "credit.deducted",
    "credit.expired",
    "credit.rolled_over",
    "credit.rollover_forfeited",
    "credit.overage_charged",
    "credit.overage_reset",
    "credit.manual_adjustment",
    "credit.balance_low",
];

/// Verify exact raw bytes first. Insert into the existing inbox and invalidate
/// any known projection in ONE commit before ACK. JSON alone is retained under
/// the existing inbox purge/dedup policy; there is no second raw payload copy.
pub async fn receive(
    service: &BillingService,
    body: &[u8],
    headers: WebhookHeaders<'_>,
) -> Result<ReceiveOutcome, BillingStoreError> {
    let event = service.provider.verify(body, headers, Utc::now())?;
    let mut tx = transaction(&service.pool).await?;
    let id:Option<i64>=sqlx::query_scalar("INSERT INTO integration_inbox_events(provider,provider_event_id,event_type,payload) VALUES($1,$2,$3,$4) ON CONFLICT(provider,provider_event_id) DO NOTHING RETURNING id")
        .bind(service.provider.namespace()).bind(&event.identity).bind(&event.event_type).bind(&event.payload).fetch_optional(&mut *tx).await?;
    if let Some(id) = id {
        if let Some(customer) = event.customer {
            let changed:Option<Uuid>=sqlx::query_scalar("UPDATE billing_accounts SET generation=generation+1,reconcile_after=now(),review_required=review_required OR $3,review_id=CASE WHEN $3 THEN $4 ELSE review_id END WHERE provider_scope=$1 AND customer_ref=$2 AND provisioning='mapped' RETURNING workspace_id")
                .bind(service.provider.namespace()).bind(customer.expose()).bind(event.adverse).bind(Uuid::new_v4()).fetch_optional(&mut *tx).await?;
            if let Some(workspace) = changed {
                sqlx::query("INSERT INTO billing_inbox_invalidations(inbox_id,account_id) SELECT $1,id FROM billing_accounts WHERE workspace_id=$2 AND provider_scope=$3").bind(id).bind(workspace).bind(service.provider.namespace()).execute(&mut *tx).await?;
                if event.adverse {
                    audit(
                        &mut tx,
                        AuditPrincipal::System,
                        WorkspaceId::new(workspace),
                        Uuid::new_v4(),
                        "review_required",
                    )
                    .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(ReceiveOutcome::Accepted(id))
    } else {
        tx.commit().await?;
        Ok(ReceiveOutcome::Duplicate)
    }
}

pub fn registry(service: BillingService) -> Result<InboxRegistry, InboxRegistryError> {
    let mut registry = InboxRegistry::new();
    let namespace = service.provider.namespace().to_owned();
    for event in EVENT_TYPES {
        let service = service.clone();
        let handler: TypedInboxHandler<Value> = Arc::new(move |payload, context| {
            let service = service.clone();
            Box::pin(async move {
                // Already authenticated at receipt; routing identity is resolved
                // only inside this configured provider/business/mode namespace.
                let customer = payload
                    .pointer("/data/customer/customer_id")
                    .or_else(|| payload.pointer("/data/customer_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        InboxHandlerError::Terminal(
                            "billing event has no supported customer identity".into(),
                        )
                    })?;
                let workspace:Option<Uuid>=sqlx::query_scalar("SELECT workspace_id FROM billing_accounts WHERE provider_scope=$1 AND customer_ref=$2 AND provisioning='mapped'").bind(service.provider.namespace()).bind(customer).fetch_optional(&service.pool).await.map_err(|_|InboxHandlerError::Transient("billing storage unavailable".into()))?;
                let Some(workspace) = workspace else {
                    // A detached (deleted-workspace) customer has no projection to
                    // update; only closure evidence is recorded. Anything else may
                    // still be a receipt before provisioning and stays pending.
                    let handled = service
                        .detached_event(customer, &payload, &context)
                        .await
                        .map_err(|_| {
                            InboxHandlerError::Transient("billing closure evidence pending".into())
                        })?;
                    return if handled {
                        Ok(())
                    } else {
                        Err(InboxHandlerError::Transient(
                            "billing customer mapping unresolved".into(),
                        ))
                    };
                };
                invalidate_replay(&service, WorkspaceId::new(workspace), &context)
                    .await
                    .map_err(|_| {
                        InboxHandlerError::Transient("billing invalidation pending".into())
                    })?;
                service
                    .reconcile_one(WorkspaceId::new(workspace), Some(&context))
                    .await
                    .map_err(|_| {
                        InboxHandlerError::Transient(
                            "billing current-state reconciliation pending".into(),
                        )
                    })
            })
        });
        registry.register(&namespace, event, handler)?;
    }
    Ok(registry)
}

// Receipt may precede customer provisioning. When mapping becomes available,
// replay must install the same once-only invalidation before any provider read.
async fn invalidate_replay(
    service: &BillingService,
    workspace: WorkspaceId,
    context: &deepref_application::inbox::InboxContext,
) -> Result<(), BillingStoreError> {
    let mut tx = transaction(&service.pool).await?;
    let live: Option<Uuid> = sqlx::query_scalar("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
        .bind(workspace.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    if live.is_none() {
        return Err(BillingStoreError::Conflict);
    }
    let account = store::account(&mut tx, workspace, service.provider.namespace())
        .await?
        .ok_or(BillingStoreError::Conflict)?;
    crate::inbox::fence_inbox_event_effect(&mut tx, context)
        .await
        .map_err(|_| BillingStoreError::Conflict)?;
    let installed=sqlx::query("INSERT INTO billing_inbox_invalidations(inbox_id,account_id) VALUES($1,$2) ON CONFLICT(inbox_id) DO NOTHING").bind(context.inbox_id).bind(account.id).execute(&mut *tx).await?.rows_affected()==1;
    if installed {
        let adverse = commercial_review_required(&context.event_type);
        sqlx::query("UPDATE billing_accounts SET generation=generation+1,reconcile_after=now(),review_required=review_required OR $2,review_id=CASE WHEN $2 THEN $3 ELSE review_id END WHERE id=$1").bind(account.id).bind(adverse).bind(Uuid::new_v4()).execute(&mut *tx).await?;
        if adverse {
            audit(
                &mut tx,
                AuditPrincipal::System,
                workspace,
                Uuid::new_v4(),
                "review_required",
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
