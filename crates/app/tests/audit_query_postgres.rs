#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Incident-response query over the administrative audit (handoff 11 Part B):
//! filtering by workspace, principal, target, event type and time range, and
//! the composite indexes that keep those filters indexed.

use chrono::{DateTime, Utc};
use deepref_app::audit_admin::{AuditEventRow, AuditQuery, query_admin_audit};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
}

async fn make_user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, $2, 'query@example.org', 'active')",
    )
    .bind(id)
    .bind(format!("user_{id}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_event(
    pool: &PgPool,
    event: &str,
    principal: Option<Uuid>,
    target: Option<Uuid>,
    workspace: Option<Uuid>,
    at: DateTime<Utc>,
) -> Uuid {
    let metadata = match event {
        "membership.removed" => serde_json::json!({ "role": "member", "grant_source": "local" }),
        _ => serde_json::json!({}),
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id, workspace_id,
            metadata, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(event)
    .bind(if principal.is_some() {
        "user"
    } else {
        "system"
    })
    .bind(principal)
    .bind(target)
    .bind(workspace)
    .bind(metadata)
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn ids(rows: &[AuditEventRow]) -> Vec<Uuid> {
    rows.iter().map(|row| row.id).collect()
}

#[tokio::test]
async fn the_query_filters_by_workspace_principal_target_event_type_and_time() {
    let Some(pool) = database().await else {
        return;
    };
    let first = make_user(&pool).await;
    let second = make_user(&pool).await;
    let workspace_a = Uuid::new_v4();
    let workspace_b = Uuid::new_v4();
    let day = |day: u32| {
        chrono::NaiveDate::from_ymd_opt(2026, 3, day)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
    };

    let in_a = insert_event(
        &pool,
        "workspace.deleted",
        None,
        None,
        Some(workspace_a),
        day(1),
    )
    .await;
    let in_b = insert_event(
        &pool,
        "workspace.deleted",
        None,
        None,
        Some(workspace_b),
        day(2),
    )
    .await;
    let login = insert_event(&pool, "auth.login", Some(first), Some(first), None, day(3)).await;
    let removal = insert_event(
        &pool,
        "membership.removed",
        Some(second),
        Some(first),
        Some(workspace_a),
        day(4),
    )
    .await;
    let completed = insert_event(
        &pool,
        "account.deletion_completed",
        None,
        Some(second),
        None,
        day(5),
    )
    .await;

    let query = |mutate: &mut dyn FnMut(&mut AuditQuery)| {
        let mut query = AuditQuery {
            limit: 100,
            ..AuditQuery::default()
        };
        mutate(&mut query);
        query
    };

    let rows = query_admin_audit(&pool, &query(&mut |q| q.workspace_id = Some(workspace_a)))
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec![removal, in_a], "workspace filter");

    let rows = query_admin_audit(&pool, &query(&mut |q| q.principal_user_id = Some(first)))
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec![login], "principal filter");

    let rows = query_admin_audit(&pool, &query(&mut |q| q.target_user_id = Some(first)))
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec![removal, login], "target filter");

    let rows = query_admin_audit(
        &pool,
        &query(&mut |q| q.event_type = Some("workspace.deleted".to_owned())),
    )
    .await
    .unwrap();
    assert_eq!(ids(&rows), vec![in_b, in_a], "event-type filter");

    let rows = query_admin_audit(
        &pool,
        &query(&mut |q| {
            q.since = Some(day(3));
            q.until = Some(day(4));
        }),
    )
    .await
    .unwrap();
    assert_eq!(ids(&rows), vec![removal, login], "time range filter");

    let rows = query_admin_audit(
        &pool,
        &query(&mut |q| {
            q.workspace_id = Some(workspace_a);
            q.event_type = Some("membership.removed".to_owned());
        }),
    )
    .await
    .unwrap();
    assert_eq!(ids(&rows), vec![removal], "combined filters");

    let rows = query_admin_audit(&pool, &query(&mut |q| q.limit = 2))
        .await
        .unwrap();
    assert_eq!(
        ids(&rows),
        vec![completed, removal],
        "newest first, bounded by the limit"
    );
}

#[tokio::test]
async fn the_incident_response_indexes_exist_and_are_valid() {
    let Some(pool) = database().await else {
        return;
    };
    for index in [
        "admin_audit_events_workspace_time_idx",
        "admin_audit_events_event_type_time_idx",
        "admin_audit_events_principal_idx",
        "admin_audit_events_target_idx",
        "admin_audit_events_occurred_idx",
    ] {
        let valid: Option<bool> = sqlx::query_scalar(
            "SELECT i.indisvalid AND i.indisready
             FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid
             WHERE c.relname = $1",
        )
        .bind(index)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(valid, Some(true), "{index}");
    }
}
