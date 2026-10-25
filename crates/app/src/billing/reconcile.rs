use super::*;
use deepref_application::inbox::{
    InboxContext, ProviderReconciler, ReconciliationError, ReconciliationOutcome,
};
use futures::future::BoxFuture;

/// Bounded policy: last proven grants survive at most 30 minutes from a
/// successful authoritative read. Failures/adverse invalidations never renew it.
const GRACE_MINUTES: i64 = 30;

impl BillingService {
    pub async fn reconcile_workspace(
        &self,
        workspace: WorkspaceId,
    ) -> Result<(), BillingStoreError> {
        self.reconcile_one(workspace, None).await
    }
    pub(super) async fn reconcile_one(
        &self,
        workspace: WorkspaceId,
        inbox: Option<&InboxContext>,
    ) -> Result<(), BillingStoreError> {
        let mut tx = transaction(&self.pool).await?;
        let live: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
                .bind(workspace.as_uuid())
                .fetch_optional(&mut *tx)
                .await?;
        if live.is_none() {
            return Err(BillingStoreError::Conflict);
        }
        let mut row = store::account(&mut tx, workspace, self.provider.namespace())
            .await?
            .ok_or(BillingStoreError::Conflict)?;
        row.generation += 1;
        sqlx::query("UPDATE billing_accounts SET generation=$2,reconcile_after=now()+interval '1 minute' WHERE id=$1").bind(row.id).bind(row.generation).execute(&mut *tx).await?;
        tx.commit().await?;
        let resolved;
        let observation;
        if let Some(customer) = row.customer.as_ref() {
            observation = Some(self.provider.observe(customer).await?);
            resolved = None;
        } else {
            resolved = match self.provider.resolve_customer(row.operation).await? {
                CustomerResolution::Found(c) => Some(c),
                CustomerResolution::Unresolved => return Err(BillingStoreError::Conflict),
            };
            observation = None;
        }
        let mut tx = transaction(&self.pool).await?;
        let live: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
                .bind(workspace.as_uuid())
                .fetch_optional(&mut *tx)
                .await?;
        if live.is_none() {
            return Err(BillingStoreError::Conflict);
        }
        let current = store::account(&mut tx, workspace, self.provider.namespace())
            .await?
            .ok_or(BillingStoreError::Conflict)?;
        if current.id != row.id
            || current.generation != row.generation
            || current.customer != row.customer
        {
            return Err(BillingStoreError::Conflict);
        }
        if let Some(context) = inbox {
            crate::inbox::fence_inbox_event_effect(&mut tx, context)
                .await
                .map_err(|_| BillingStoreError::Conflict)?;
        }
        if let Some(customer) = resolved {
            store::map(&mut tx, &row, &customer).await?;
            audit(
                &mut tx,
                AuditPrincipal::System,
                workspace,
                row.operation,
                "customer_mapped",
            )
            .await?;
        }
        if let Some(observation) = observation {
            let new_review = observation
                .review
                .filter(|id| Some(*id) != current.resolved_review);
            if let Some(review) = new_review {
                sqlx::query(
                    "UPDATE billing_accounts SET review_required=true,review_id=$2 WHERE id=$1",
                )
                .bind(row.id)
                .bind(review)
                .execute(&mut *tx)
                .await?;
                if !current.held {
                    audit(
                        &mut tx,
                        AuditPrincipal::System,
                        workspace,
                        review,
                        "review_required",
                    )
                    .await?;
                }
            }
            // An active subscription alone cannot resolve a refund/dispute/credit
            // review. Preserve the old finite evidence, never extend its grace.
            if (!current.held && new_review.is_none())
                || observation.state != SubscriptionState::Active
            {
                let references = observation
                    .reference
                    .as_ref()
                    .map(SubscriptionReference::expose);
                let projection = BillingProjection::Subscription {
                    state: observation.state,
                    entitlements: observation.entitlements,
                    observed_at: observation.observed_at,
                    valid_until: observation.observed_at + chrono::Duration::minutes(GRACE_MINUTES),
                };
                sqlx::query(
                    "UPDATE billing_accounts SET projection=$2,subscription_ref=$3,product_ref=$4,reconciled_at=now() WHERE id=$1",
                )
                .bind(row.id)
                .bind(serde_json::to_value(projection).map_err(|_| BillingStoreError::Projection)?)
                .bind(references.map(|(subscription,_)|subscription))
                .bind(references.map(|(_,product)|product))
                .execute(&mut *tx)
                .await?;
                audit(
                    &mut tx,
                    AuditPrincipal::System,
                    workspace,
                    Uuid::new_v4(),
                    "projection_updated",
                )
                .await?;
            }
        }
        if let Some(context) = inbox {
            crate::inbox::complete_inbox_event_in_transaction(&mut tx, context)
                .await
                .map_err(|_| BillingStoreError::Conflict)?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Explicit operator recovery after reviewing relevant CURRENT payment,
    /// dispute/refund/credit evidence. Evidence UUID points at the operator's
    /// restricted review record; a subscription.active event is insufficient.
    /// Compare review identity so a newer invalidation cannot be cleared.
    pub async fn resolve_review(
        &self,
        workspace: WorkspaceId,
        expected_review: Uuid,
        evidence: Uuid,
    ) -> Result<(), BillingStoreError> {
        let mut tx = transaction(&self.pool).await?;
        let live: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
                .bind(workspace.as_uuid())
                .fetch_optional(&mut *tx)
                .await?;
        if live.is_none() {
            return Err(BillingStoreError::Conflict);
        }
        let updated=sqlx::query("UPDATE billing_accounts SET review_required=false,resolved_review_id=review_id,review_id=NULL,generation=generation+1,reconcile_after=now() WHERE workspace_id=$1 AND provider_scope=$2 AND review_id=$3 AND provisioning='mapped'").bind(workspace.as_uuid()).bind(self.provider.namespace()).bind(expected_review).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(BillingStoreError::Conflict);
        }
        audit(
            &mut tx,
            AuditPrincipal::System,
            workspace,
            evidence,
            "review_resolved",
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

pub struct BillingReconciler {
    service: BillingService,
    namespace: String,
}
impl BillingReconciler {
    pub fn new(service: BillingService) -> Self {
        Self {
            namespace: service.provider.namespace().into(),
            service,
        }
    }
}
impl ProviderReconciler for BillingReconciler {
    fn provider(&self) -> &str {
        &self.namespace
    }
    fn reconcile(&self) -> BoxFuture<'static, Result<ReconciliationOutcome, ReconciliationError>> {
        let service = self.service.clone();
        Box::pin(async move {
            let fail = || ReconciliationError::Failed {
                provider: "dodo".into(),
                message: "billing reconciliation requires attention".into(),
            };
            let workspaces:Vec<Uuid>=sqlx::query_scalar("SELECT workspace_id FROM billing_accounts WHERE provider_scope=$1 AND provisioning<>'detached' AND reconcile_after<=now() ORDER BY reconcile_after,id LIMIT 16").bind(service.provider.namespace()).fetch_all(&service.pool).await.map_err(|_|fail())?;
            let mut outcome = ReconciliationOutcome::default();
            let mut failed = false;
            for workspace in workspaces {
                outcome.checked += 1;
                match service
                    .reconcile_workspace(WorkspaceId::new(workspace))
                    .await
                {
                    Ok(()) => outcome.repaired += 1,
                    Err(_) => failed = true,
                }
            }
            if failed { Err(fail()) } else { Ok(outcome) }
        })
    }
}
