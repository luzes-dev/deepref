#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Audit CHECK snapshots must preserve both the released AI contracts and 11B.

use deepref_app::MIGRATOR;
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

async fn empty_database() -> Option<PgPool> {
    let url = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await?;
    Some(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap(),
    )
}

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,$2,'audit@example.test','active')")
        .bind(id).bind(format!("audit-{id}")).execute(pool).await.unwrap();
    id
}

async fn insert(
    pool: &PgPool,
    event: &str,
    principal: Option<Uuid>,
    target: Option<Uuid>,
    scope: (Option<Uuid>, Option<Uuid>),
    metadata: Value,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO admin_audit_events(id,event_type,principal_kind,principal_user_id,target_user_id,workspace_id,project_id,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(event).bind(if principal.is_some() { "user" } else { "system" })
        .bind(principal).bind(target).bind(scope.0).bind(scope.1).bind(metadata)
        .execute(pool).await?;
    Ok(id)
}

async fn ai_events(pool: &PgPool, principal: Uuid) -> Vec<Uuid> {
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    let mut ids = Vec::new();
    for (event, scope, metadata) in [
        (
            "ai.usage_limit_changed",
            (None, None),
            json!({"scope_kind":"platform","old_limit_micros":0,"new_limit_micros":100}),
        ),
        (
            "ai.usage_limit_changed",
            (Some(workspace), None),
            json!({"scope_kind":"workspace","old_limit_micros":0,"new_limit_micros":100}),
        ),
        (
            "ai.usage_limit_changed",
            (Some(workspace), Some(project)),
            json!({"scope_kind":"project","old_limit_micros":0,"new_limit_micros":100}),
        ),
        (
            "ai.usage_overage_waived",
            (Some(workspace), None),
            json!({"overage_micros":1}),
        ),
        (
            "ai.hard_caps_enabled",
            (None, None),
            json!({"quiet_window_seconds":300}),
        ),
        // Both shapes remain valid for previous-release writers.
        ("ai.hard_caps_disabled", (None, None), json!({})),
        (
            "ai.hard_caps_disabled",
            (None, None),
            json!({"parked_jobs":0}),
        ),
        (
            "ai.usage_export_enabled",
            (None, None),
            json!({"quiet_window_seconds":300}),
        ),
        (
            "ai.usage_export_disabled",
            (None, None),
            json!({"parked_jobs":0}),
        ),
        (
            "ai.usage_reservation_resolved",
            (Some(workspace), Some(project)),
            json!({"reservation_id":Uuid::new_v4(),"resolution":"settle","cost_micros":0}),
        ),
        (
            "ai.usage_reservation_resolved",
            (Some(workspace), None),
            json!({"reservation_id":Uuid::new_v4(),"resolution":"void","reason":"operator verified"}),
        ),
    ] {
        ids.push(
            insert(pool, event, Some(principal), None, scope, metadata)
                .await
                .expect(event),
        );
    }
    ids
}

async fn security_events_and_rejections(pool: &PgPool, principal: Uuid) {
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    for (event, actor, target, scope, metadata) in [
        (
            "auth.rejected",
            None,
            None,
            (None, None),
            json!({"stage":"callback","reason":"invalid_state","window":"123"}),
        ),
        (
            "account.deletion_requested",
            Some(principal),
            Some(principal),
            (None, None),
            json!({}),
        ),
        (
            "account.deletion_cancelled",
            Some(principal),
            Some(principal),
            (None, None),
            json!({}),
        ),
        (
            "account.deletion_completed",
            None,
            Some(principal),
            (None, None),
            json!({}),
        ),
        (
            "user.deactivated",
            None,
            Some(principal),
            (None, None),
            json!({"source":"account_deletion"}),
        ),
        (
            "user.deactivated",
            None,
            Some(principal),
            (None, None),
            json!({"source":"provider_event"}),
        ),
        (
            "project.created",
            Some(principal),
            None,
            (Some(workspace), Some(project)),
            json!({}),
        ),
        (
            "project.deleted",
            Some(principal),
            None,
            (Some(workspace), Some(project)),
            json!({}),
        ),
        (
            "audit.purged",
            None,
            None,
            (None, None),
            json!({"rows_purged":0}),
        ),
    ] {
        insert(pool, event, actor, target, scope, metadata)
            .await
            .expect(event);
    }
    for (event, metadata) in [
        (
            "ai.usage_limit_changed",
            json!({"scope_kind":"platform","old_limit_micros":0,"new_limit_micros":-1}),
        ),
        ("ai.hard_caps_disabled", json!({"parked_jobs":-1})),
        (
            "ai.usage_export_disabled",
            json!({"parked_jobs":0,"secret":"forbidden"}),
        ),
        ("ai.usage_export_enabled", json!({"quiet_window_seconds":0})),
        (
            "auth.rejected",
            json!({"stage":"callback","reason":"caller-controlled","window":"124"}),
        ),
        ("audit.purged", json!({"rows_purged":-1})),
        ("account.deletion_requested", json!({"secret":"forbidden"})),
    ] {
        let target = (event == "account.deletion_requested").then_some(principal);
        let actor = (!matches!(event, "auth.rejected" | "audit.purged")).then_some(principal);
        let error = insert(pool, event, actor, target, (None, None), metadata)
            .await
            .expect_err(event);
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23514"),
            "{event}"
        );
    }
    let validated: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='admin_audit_events'::regclass AND conname IN ('admin_audit_events_event_type_check','admin_audit_events_check') AND convalidated")
        .fetch_one(pool).await.unwrap();
    assert_eq!(validated, 2);
}

#[tokio::test]
async fn fresh_database_accepts_ai_and_security_audit_without_loosening_metadata() {
    let Some(pool) = empty_database().await else {
        return;
    };
    MIGRATOR.run(&pool).await.expect("fresh migrations");
    let principal = user(&pool).await;
    ai_events(&pool, principal).await;
    security_events_and_rejections(&pool, principal).await;
}

#[tokio::test]
async fn current_main_with_ai_audit_rows_upgrades_to_security_audit() {
    let Some(pool) = empty_database().await else {
        return;
    };
    // Exactly the released migration set through 0140, excluding unmerged 11B.
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|m| !(111..=117).contains(&m.version) && m.version <= 140)
            .cloned()
            .collect(),
    )
    .run(&pool)
    .await
    .expect("current main migrations");
    let principal = user(&pool).await;
    let existing_ids = ai_events(&pool, principal).await;
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade with existing AI audit rows");
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE id=ANY($1)")
            .bind(&existing_ids)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(usize::try_from(retained).unwrap(), existing_ids.len());
    ai_events(&pool, principal).await;
    security_events_and_rejections(&pool, principal).await;
}
