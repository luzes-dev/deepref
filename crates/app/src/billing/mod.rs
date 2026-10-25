//! Durable local billing orchestration. Provider effects run without DB locks;
//! every response is fenced against current tenant, permission and generation.
mod reconcile;
mod store;
mod webhooks;
pub use reconcile::BillingReconciler;
pub use webhooks::{receive, registry};

use crate::{
    authorization::TransactionAuthorizationStore,
    identity::{AuditPrincipal, AuditRequest, insert_admin_audit},
};
use chrono::Utc;
use deepref_application::{
    authorization::{Authorizer, PolicyDecisionPoint},
    billing::*,
};
use deepref_domain::{
    Principal, UserId, WorkspaceId,
    authorization::{Permission, Resource},
    billing::*,
};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum BillingStoreError {
    #[error("billing access denied")]
    Denied,
    #[error("billing evidence requires reconciliation")]
    Conflict,
    #[error(transparent)]
    Provider(#[from] BillingError),
    #[error("billing storage unavailable")]
    Database(#[from] sqlx::Error),
    #[error("invalid local billing projection")]
    Projection,
}

#[derive(Clone)]
pub struct BillingService {
    pool: PgPool,
    provider: Arc<dyn BillingProvider>,
}
impl BillingService {
    pub fn new(pool: PgPool, provider: Arc<dyn BillingProvider>) -> Self {
        Self { pool, provider }
    }
    pub fn provider(&self) -> &dyn BillingProvider {
        self.provider.as_ref()
    }

    pub async fn read(
        &self,
        actor: UserId,
        workspace: WorkspaceId,
    ) -> Result<BillingView, BillingStoreError> {
        let mut tx = manager(&self.pool, actor, workspace).await?;
        let view = store::view(&mut tx, workspace, self.provider.namespace()).await?;
        tx.commit().await?;
        Ok(view)
    }

    /// A committed intent dispatches at most once. A crash at any point after
    /// intent commit leaves actionable ambiguity; zero results never retry POST.
    pub async fn provision(
        &self,
        actor: UserId,
        workspace: WorkspaceId,
        contact: BillingContact,
    ) -> Result<(), BillingStoreError> {
        let mut tx = manager(&self.pool, actor, workspace).await?;
        let existing = store::account(&mut tx, workspace, self.provider.namespace()).await?;
        if let Some(row) = existing {
            tx.commit().await?;
            return if row.customer.is_some() {
                Ok(())
            } else {
                Err(BillingStoreError::Conflict)
            };
        }
        let id = Uuid::new_v4();
        let operation = Uuid::new_v4();
        sqlx::query("DELETE FROM billing_free_enrollments WHERE workspace_id=$1")
            .bind(workspace.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO billing_accounts(id,workspace_id,provider_scope,operation_id,provisioning) VALUES ($1,$2,$3,$4,'ambiguous')")
            .bind(id).bind(workspace.as_uuid()).bind(self.provider.namespace()).bind(operation).execute(&mut *tx).await?;
        audit(
            &mut tx,
            AuditPrincipal::User(actor),
            workspace,
            operation,
            "provisioning_started",
        )
        .await?;
        tx.commit().await?;
        let customer = self
            .provider
            .create_customer(CustomerIntent {
                workspace,
                operation,
                contact: &contact,
            })
            .await?;
        // Membership/account changes during HTTP cannot commit the returned mapping.
        let mut tx = manager(&self.pool, actor, workspace).await?;
        let Some(row) = store::account(&mut tx, workspace, self.provider.namespace()).await? else {
            return Err(BillingStoreError::Conflict);
        };
        if row.id != id || row.generation != 0 || row.operation != operation {
            return Err(BillingStoreError::Conflict);
        }
        store::map(&mut tx, &row, &customer).await?;
        audit(
            &mut tx,
            AuditPrincipal::User(actor),
            workspace,
            operation,
            "customer_mapped",
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn checkout(
        &self,
        actor: UserId,
        workspace: WorkspaceId,
        offering: Offering,
        return_path: &str,
    ) -> Result<HostedLink, BillingStoreError> {
        self.link(actor, workspace, Some(offering), return_path)
            .await
    }
    pub async fn portal(
        &self,
        actor: UserId,
        workspace: WorkspaceId,
        return_path: &str,
    ) -> Result<HostedLink, BillingStoreError> {
        self.link(actor, workspace, None, return_path).await
    }
    async fn link(
        &self,
        actor: UserId,
        workspace: WorkspaceId,
        offering: Option<Offering>,
        return_path: &str,
    ) -> Result<HostedLink, BillingStoreError> {
        let mut tx = manager(&self.pool, actor, workspace).await?;
        let row = store::account(&mut tx, workspace, self.provider.namespace())
            .await?
            .ok_or(BillingStoreError::Conflict)?;
        let customer = row.customer.as_ref().ok_or(BillingStoreError::Conflict)?;
        tx.commit().await?;
        let link = match offering {
            Some(offering) => {
                self.provider
                    .checkout(customer, offering, return_path)
                    .await?
            }
            None => self.provider.portal(customer, return_path).await?,
        };
        let mut tx = manager(&self.pool, actor, workspace).await?;
        let current = store::account(&mut tx, workspace, self.provider.namespace())
            .await?
            .ok_or(BillingStoreError::Conflict)?;
        if current.id != row.id
            || current.generation != row.generation
            || current.customer != row.customer
        {
            return Err(BillingStoreError::Conflict);
        }
        audit(
            &mut tx,
            AuditPrincipal::User(actor),
            workspace,
            Uuid::new_v4(),
            if offering.is_some() {
                "checkout_created"
            } else {
                "portal_created"
            },
        )
        .await?;
        tx.commit().await?;
        Ok(link)
    }
}

/// Export-safe local fields. No provider reference, inbox body, or capability.
#[derive(Debug, serde::Serialize)]
pub struct BillingView {
    pub effective: EffectiveBilling,
    pub review_required: bool,
    pub provisioning_ambiguous: bool,
    pub reconciled_at: Option<chrono::DateTime<Utc>>,
}

async fn transaction(pool: &PgPool) -> Result<Transaction<'_, Postgres>, BillingStoreError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='1500ms'")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
async fn manager(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
) -> Result<Transaction<'_, Postgres>, BillingStoreError> {
    let mut tx = transaction(pool).await?;
    // Canonical account deletion lock order, no nested pool acquisition.
    let user: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR SHARE")
        .bind(actor.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    let tenant: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
            .bind(workspace.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
    let member: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workspace_memberships WHERE user_id=$1 AND workspace_id=$2 FOR SHARE",
    )
    .bind(actor.as_uuid())
    .bind(workspace.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if user.is_none() || tenant.is_none() || member.is_none() {
        return Err(BillingStoreError::Denied);
    }
    PolicyDecisionPoint::new(TransactionAuthorizationStore::new(&mut tx))
        .require(
            &Principal::user(actor),
            Permission::WorkspaceBilling,
            Resource::Workspace(workspace),
        )
        .await
        .map_err(|e| match e {
            deepref_application::authorization::AuthzError::Denied(_) => BillingStoreError::Denied,
            _ => BillingStoreError::Projection,
        })?;
    Ok(tx)
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    principal: AuditPrincipal,
    workspace: WorkspaceId,
    operation: Uuid,
    reason: &str,
) -> Result<(), BillingStoreError> {
    insert_admin_audit(
        tx,
        "billing.changed",
        principal,
        None,
        Some(workspace.as_uuid()),
        None,
        serde_json::json!({"operation_id":operation,"reason":reason}),
        AuditRequest::default(),
    )
    .await?;
    Ok(())
}

/// Explicit product-policy operation, never inferred by provider reconciliation.
/// Caller is an operator selecting reviewed local free limits; no HTTP enrollment
/// endpoint is exposed. Cannot classify paid/provisioning history as free.
pub async fn enroll_free(
    pool: &PgPool,
    workspace: WorkspaceId,
    entitlements: Entitlements,
) -> Result<(), BillingStoreError> {
    let mut tx = transaction(pool).await?;
    let live: Option<Uuid> = sqlx::query_scalar("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
        .bind(workspace.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    if live.is_none() {
        return Err(BillingStoreError::Denied);
    }
    let history: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM billing_accounts WHERE workspace_id=$1)")
            .bind(workspace.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
    if history {
        return Err(BillingStoreError::Conflict);
    }
    let operation = Uuid::new_v4();
    sqlx::query("INSERT INTO billing_free_enrollments(workspace_id,operation_id,entitlements) VALUES($1,$2,$3) ON CONFLICT(workspace_id) DO NOTHING").bind(workspace.as_uuid()).bind(operation).bind(serde_json::to_value(entitlements).map_err(|_| BillingStoreError::Projection)?).execute(&mut *tx).await?;
    audit(
        &mut tx,
        AuditPrincipal::System,
        workspace,
        operation,
        "free_enrolled",
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
