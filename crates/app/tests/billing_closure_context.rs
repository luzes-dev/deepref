//! Restricted original-context capture, not evidence of provider closure.
//! All provider references are local literals; no provider adapter is invoked.
use anyhow::{Context, ensure};
use deepref_app::{MIGRATOR, identity::AuditRequest};
use deepref_domain::{UserId, WorkspaceId};
use serde_json::Value;
use sqlx::{PgPool, Row, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

async fn workspace(pool: &PgPool) -> anyhow::Result<(UserId, WorkspaceId)> {
    let user = UserId::new(Uuid::new_v4());
    let workspace = WorkspaceId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,$2,'closure@fixture.example','active')")
        .bind(user.as_uuid()).bind(format!("closure-{}", user.as_uuid())).execute(pool).await?;
    sqlx::query(
        "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','closure fixture',$2)",
    )
    .bind(workspace.as_uuid())
    .bind(format!("closure-{}", workspace.as_uuid()))
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
        .bind(Uuid::new_v4()).bind(workspace.as_uuid()).bind(user.as_uuid()).execute(pool).await?;
    Ok((user, workspace))
}

async fn mapping(pool: &PgPool, workspace: WorkspaceId, mapped: bool) -> anyhow::Result<Uuid> {
    let account = Uuid::new_v4();
    sqlx::query("INSERT INTO billing_accounts(id,workspace_id,provider_scope,operation_id,customer_ref,subscription_ref,product_ref,provisioning) VALUES($1,$2,'dodo:test_mode:original_business',$3,$4,$5,$6,$7)")
        .bind(account).bind(workspace.as_uuid()).bind(Uuid::new_v4())
        .bind(mapped.then_some("cus_original")).bind(mapped.then_some("sub_original"))
        .bind(mapped.then_some("pdt_original")).bind(if mapped {"mapped"} else {"ambiguous"})
        .execute(pool).await?;
    Ok(account)
}

async fn captured(pool: &PgPool, account: Uuid) -> anyhow::Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('workspace',closure_workspace_id,'customer',closure_customer_ref,'subscription',closure_subscription_ref,'at',closure_captured_at,'scope',provider_scope) FROM billing_accounts WHERE id=$1")
        .bind(account).fetch_one(pool).await?)
}

fn check_violation(
    result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
) -> anyhow::Result<()> {
    let error = result
        .err()
        .context("closure mutation unexpectedly succeeded")?;
    ensure!(
        error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23514"),
        "expected closure invariant rejection, not a setup or connection failure"
    );
    Ok(())
}

#[tokio::test]
async fn original_context_is_immutable_and_preserves_existing_five_year_expiry()
-> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let (user, tenant) = workspace(&pool).await?;
    let account = mapping(&pool, tenant, true).await?;
    let before: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await?;
    deepref_app::workspaces::delete_workspace(&pool, user, tenant, AuditRequest::default()).await?;
    let after: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await?;
    let row = sqlx::query("SELECT closure_workspace_id,closure_customer_ref,closure_subscription_ref,closure_captured_at,provider_scope,purge_after=detached_at+interval '5 years' AS existing_expiry,workspace_id IS NULL AND customer_ref IS NULL AND subscription_ref IS NULL AND product_ref IS NULL AND provisioning='detached' AS detached FROM billing_accounts WHERE id=$1")
        .bind(account).fetch_one(&pool).await?;
    ensure!(
        row.try_get::<Option<Uuid>, _>("closure_workspace_id")? == Some(tenant.as_uuid()),
        "original tenant context changed"
    );
    ensure!(
        row.try_get::<Option<String>, _>("closure_customer_ref")?
            .as_deref()
            == Some("cus_original"),
        "original customer was not captured"
    );
    ensure!(
        row.try_get::<Option<String>, _>("closure_subscription_ref")?
            .as_deref()
            == Some("sub_original"),
        "original subscription was not captured"
    );
    ensure!(
        row.try_get::<String, _>("provider_scope")? == "dodo:test_mode:original_business",
        "original provider scope changed"
    );
    ensure!(
        row.try_get::<bool, _>("existing_expiry")? && row.try_get::<bool, _>("detached")?,
        "existing detachment or five-year expiry changed"
    );
    let at: chrono::DateTime<chrono::Utc> = row.try_get("closure_captured_at")?;
    ensure!(
        before <= at && at <= after,
        "capture did not use actual detachment time"
    );
    let original = captured(&pool, account).await?;
    // An ordinary later reconciliation/review update must preserve context.
    sqlx::query(r#"UPDATE billing_accounts SET projection='{"kind":"unknown"}',review_required=true WHERE id=$1"#)
        .bind(account).execute(&pool).await?;
    for statement in [
        "UPDATE billing_accounts SET closure_workspace_id=gen_random_uuid() WHERE id=$1",
        "UPDATE billing_accounts SET closure_customer_ref='cus_later' WHERE id=$1",
        "UPDATE billing_accounts SET closure_subscription_ref='sub_later' WHERE id=$1",
        "UPDATE billing_accounts SET closure_captured_at=closure_captured_at+interval '1 second' WHERE id=$1",
        "UPDATE billing_accounts SET provider_scope='dodo:test_mode:another_business' WHERE id=$1",
        "UPDATE billing_accounts SET closure_workspace_id=NULL,closure_customer_ref=NULL,closure_subscription_ref=NULL,closure_captured_at=NULL WHERE id=$1",
    ] {
        check_violation(
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .bind(account)
                .execute(&pool)
                .await,
        )?;
        ensure!(
            captured(&pool, account).await? == original,
            "rejected mutation changed original context"
        );
    }
    Ok(())
}

#[tokio::test]
async fn pre_capture_release_detachment_stays_unknown_after_upgrade() -> anyhow::Result<()> {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < 193)
            .cloned()
            .collect(),
    )
    .run(&pool)
    .await?;
    let (user, tenant) = workspace(&pool).await?;
    let account = mapping(&pool, tenant, true).await?;
    deepref_app::workspaces::delete_workspace(&pool, user, tenant, AuditRequest::default()).await?;
    let original: Value =
        sqlx::query_scalar("SELECT to_jsonb(b) FROM billing_accounts b WHERE id=$1")
            .bind(account)
            .fetch_one(&pool)
            .await?;
    MIGRATOR.run(&pool).await?;
    let row = sqlx::query("SELECT to_jsonb(b)-ARRAY['closure_workspace_id','closure_customer_ref','closure_subscription_ref','closure_captured_at'] AS old_fields,closure_captured_at IS NULL AND closure_workspace_id IS NULL AND closure_customer_ref IS NULL AND closure_subscription_ref IS NULL AS unknown FROM billing_accounts b WHERE id=$1")
        .bind(account).fetch_one(&pool).await?;
    ensure!(
        row.try_get::<Value, _>("old_fields")? == original,
        "upgrade rewrote retained historical billing fields"
    );
    ensure!(
        row.try_get::<bool, _>("unknown")?,
        "upgrade invented historical closure context"
    );
    check_violation(sqlx::query("UPDATE billing_accounts SET closure_captured_at=clock_timestamp(),closure_customer_ref='cus_invented' WHERE id=$1")
        .bind(account).execute(&pool).await)?;
    let still_unknown: bool = sqlx::query_scalar("SELECT closure_captured_at IS NULL AND closure_customer_ref IS NULL FROM billing_accounts WHERE id=$1")
        .bind(account).fetch_one(&pool).await?;
    ensure!(still_unknown, "rejected historical fill invented context");
    Ok(())
}

#[tokio::test]
async fn ambiguous_original_account_does_not_invent_provider_references() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let (user, tenant) = workspace(&pool).await?;
    let account = mapping(&pool, tenant, false).await?;
    deepref_app::workspaces::delete_workspace(&pool, user, tenant, AuditRequest::default()).await?;
    let row = sqlx::query("SELECT closure_workspace_id,closure_captured_at IS NOT NULL AS captured,closure_customer_ref IS NULL AND closure_subscription_ref IS NULL AS missing,provisioning='detached' AND review_required AS held FROM billing_accounts WHERE id=$1")
        .bind(account).fetch_one(&pool).await?;
    ensure!(
        row.try_get::<Option<Uuid>, _>("closure_workspace_id")? == Some(tenant.as_uuid()),
        "missing original tenant identity"
    );
    ensure!(
        row.try_get::<bool, _>("captured")?
            && row.try_get::<bool, _>("missing")?
            && row.try_get::<bool, _>("held")?,
        "ambiguous detachment invented provider context or cleared review"
    );
    Ok(())
}

#[tokio::test]
async fn asserted_context_cannot_be_inserted_as_capture_evidence() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let account = Uuid::new_v4();
    check_violation(sqlx::query("INSERT INTO billing_accounts(id,provider_scope,operation_id,provisioning,detached_at,purge_after,closure_customer_ref,closure_captured_at) VALUES($1,'dodo:test_mode:original_business',$2,'detached',now(),now()+interval '5 years','cus_asserted',clock_timestamp())")
        .bind(account).bind(Uuid::new_v4()).execute(&pool).await)?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM billing_accounts WHERE id=$1)")
            .bind(account)
            .fetch_one(&pool)
            .await?;
    ensure!(!exists, "rejected invented capture left a billing row");
    Ok(())
}
