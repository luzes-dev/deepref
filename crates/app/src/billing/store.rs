use super::*;

pub(super) struct Account {
    pub id: Uuid,
    pub operation: Uuid,
    pub generation: i64,
    pub customer: Option<CustomerReference>,
    pub held: bool,
    pub resolved_review: Option<Uuid>,
}

pub(super) async fn account(
    tx: &mut Transaction<'_, Postgres>,
    workspace: WorkspaceId,
    scope: &str,
) -> Result<Option<Account>, BillingStoreError> {
    let row=sqlx::query("SELECT id,operation_id,generation,customer_ref,review_required,resolved_review_id FROM billing_accounts WHERE workspace_id=$1 AND provider_scope=$2 AND provisioning <> 'detached' FOR UPDATE").bind(workspace.as_uuid()).bind(scope).fetch_optional(&mut **tx).await?;
    row.map(|r| {
        Ok(Account {
            id: r.try_get("id")?,
            operation: r.try_get("operation_id")?,
            generation: r.try_get("generation")?,
            held: r.try_get("review_required")?,
            resolved_review: r.try_get("resolved_review_id")?,
            customer: r
                .try_get::<Option<String>, _>("customer_ref")?
                .map(CustomerReference::new)
                .transpose()?,
        })
    })
    .transpose()
}
pub(super) async fn map(
    tx: &mut Transaction<'_, Postgres>,
    row: &Account,
    customer: &CustomerReference,
) -> Result<(), BillingStoreError> {
    sqlx::query("UPDATE billing_accounts SET customer_ref=$2,provisioning='mapped',generation=generation+1,reconcile_after=now() WHERE id=$1").bind(row.id).bind(customer.expose()).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn view(
    tx: &mut Transaction<'_, Postgres>,
    workspace: WorkspaceId,
    scope: &str,
) -> Result<BillingView, BillingStoreError> {
    let row=sqlx::query("SELECT projection,review_required,provisioning,reconciled_at FROM billing_accounts WHERE workspace_id=$1 AND provider_scope=$2 AND provisioning<>'detached'").bind(workspace.as_uuid()).bind(scope).fetch_optional(&mut **tx).await?;
    if let Some(r) = row {
        let projection: BillingProjection = serde_json::from_value(r.try_get("projection")?)
            .map_err(|_| BillingStoreError::Projection)?;
        return Ok(BillingView {
            effective: projection.effective(Utc::now()),
            review_required: r.try_get("review_required")?,
            provisioning_ambiguous: r.try_get::<String, _>("provisioning")? == "ambiguous",
            reconciled_at: r.try_get("reconciled_at")?,
        });
    }
    let free = sqlx::query(
        "SELECT entitlements,enrolled_at FROM billing_free_enrollments WHERE workspace_id=$1",
    )
    .bind(workspace.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    let projection = match free {
        None => BillingProjection::Unknown,
        Some(r) => BillingProjection::Free {
            enrolled_at: r.try_get("enrolled_at")?,
            entitlements: serde_json::from_value(r.try_get("entitlements")?)
                .map_err(|_| BillingStoreError::Projection)?,
        },
    };
    Ok(BillingView {
        effective: projection.effective(Utc::now()),
        review_required: false,
        provisioning_ambiguous: false,
        reconciled_at: None,
    })
}
