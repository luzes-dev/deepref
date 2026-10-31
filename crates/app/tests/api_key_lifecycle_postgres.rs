#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Resource ownership regressions: the central PDP must use the lifecycle
//! transaction, including when all pool connections are already occupied.
use std::{sync::Arc, time::Duration};

use deepref_app::{
    api_keys::{self, ApiKeyError, KeySpec},
    identity::AuditRequest,
};
use deepref_domain::{ApiKeyId, UserId, WorkspaceId};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::{sync::Barrier, task::JoinSet};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(5);
const AUDIT: AuditRequest<'_> = AuditRequest {
    request_id: None,
    session_ref: None,
};

async fn database(connections: u32) -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", connections).await
}

async fn seed(pool: &PgPool) -> (UserId, WorkspaceId) {
    let user = UserId::new(Uuid::new_v4());
    let workspace = WorkspaceId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO users (id,workos_user_id,primary_email,status) VALUES ($1,$2,'pool@example.test','active')")
        .bind(user.as_uuid()).bind(format!("pool-{}", user.as_uuid())).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO workspaces (id,kind,name,slug) VALUES ($1,'organization','pool test',$2)",
    )
    .bind(workspace.as_uuid())
    .bind(format!("pool-{}", workspace.as_uuid()))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspace_memberships (id,workspace_id,user_id,role,status,grant_source) VALUES ($1,$2,$3,'owner','active','local')")
        .bind(Uuid::new_v4()).bind(workspace.as_uuid()).bind(user.as_uuid()).execute(pool).await.unwrap();
    (user, workspace)
}

fn spec() -> KeySpec {
    KeySpec::parse(
        "pool test".into(),
        vec!["project:view".into()],
        None,
        chrono::Utc::now(),
    )
    .unwrap()
}

#[tokio::test]
async fn one_connection_supports_lifecycle_revoke_and_rotation_rollback() {
    let Some(pool) = database(1).await else {
        return;
    };
    let (user, workspace) = seed(&pool).await;
    let issued = tokio::time::timeout(
        DEADLINE,
        api_keys::create(&pool, user, workspace, spec(), AUDIT),
    )
    .await
    .expect("create must not wait for a second pool connection")
    .unwrap();
    tokio::time::timeout(DEADLINE, async {
        let id = ApiKeyId::new(issued.record.id);
        let listed = api_keys::list(&pool, user, workspace).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].revoked_at.is_none());
        // Fail after the old key was revoked and the replacement was inserted:
        // both the new row and every lifecycle audit insert must roll back.
        sqlx::raw_sql("CREATE FUNCTION reject_test_key_rotation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_type='api_key.rotated' THEN RAISE EXCEPTION 'forced test rotation failure'; END IF; RETURN NEW; END $$;
            CREATE TRIGGER reject_test_key_rotation BEFORE INSERT ON admin_audit_events FOR EACH ROW EXECUTE FUNCTION reject_test_key_rotation();")
            .execute(&pool).await.unwrap();
        assert!(matches!(api_keys::rotate(&pool, user, workspace, id, AUDIT).await, Err(ApiKeyError::Database(_))));
        let listed = api_keys::list(&pool, user, workspace).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id.as_uuid());
        assert!(listed[0].revoked_at.is_none());
        assert!(api_keys::authenticate(&pool, issued.secret.expose()).await.unwrap().is_some());
        let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE event_type LIKE 'api_key.%'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(audit_count, 1, "failed rotation rolls back every audit insert");
        sqlx::raw_sql("DROP TRIGGER reject_test_key_rotation ON admin_audit_events; DROP FUNCTION reject_test_key_rotation();")
            .execute(&pool).await.unwrap();
        let rotated = api_keys::rotate(&pool, user, workspace, id, AUDIT).await.unwrap();
        assert!(api_keys::authenticate(&pool, issued.secret.expose()).await.unwrap().is_none());
        assert!(api_keys::authenticate(&pool, rotated.secret.expose()).await.unwrap().is_some());
        api_keys::revoke(&pool, user, workspace, ApiKeyId::new(rotated.record.id), AUDIT).await.unwrap();
        api_keys::revoke(&pool, user, workspace, ApiKeyId::new(rotated.record.id), AUDIT).await.unwrap();
        assert!(api_keys::authenticate(&pool, rotated.secret.expose()).await.unwrap().is_none());
        assert!(api_keys::list(&pool, user, workspace).await.unwrap().iter().all(|key| key.revoked_at.is_some()));
        sqlx::query("UPDATE workspace_memberships SET role='member' WHERE user_id=$1")
            .bind(user.as_uuid()).execute(&pool).await.unwrap();
        assert!(matches!(api_keys::list(&pool, user, workspace).await, Err(ApiKeyError::Denied)));
        assert!(matches!(api_keys::create(&pool, user, workspace, spec(), AUDIT).await, Err(ApiKeyError::Denied)));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 2, "denial rolls back and releases the sole connection");
    }).await.expect("every lifecycle and rollback path must complete on one connection");
    pool.close().await;
}

#[tokio::test]
async fn saturated_same_workspace_lifecycle_burst_completes_without_second_connections() {
    const CONCURRENCY: u32 = 4;
    let Some(pool) = database(CONCURRENCY).await else {
        return;
    };
    let (user, workspace) = seed(&pool).await;
    // Control connections deliberately do not come from the saturated pool.
    let options = pool.connect_options();
    let mut blocker = PgConnection::connect_with(&options).await.unwrap();
    let mut observer = PgConnection::connect_with(&options).await.unwrap();
    let mut blocking = blocker.begin().await.unwrap();
    sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
        .bind(workspace.as_uuid())
        .fetch_one(&mut *blocking)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(CONCURRENCY as usize + 1));
    let mut tasks = JoinSet::new();
    for _ in 0..CONCURRENCY {
        let pool = pool.clone();
        let barrier = Arc::clone(&barrier);
        tasks.spawn(async move {
            barrier.wait().await;
            let key = api_keys::create(&pool, user, workspace, spec(), AUDIT).await?;
            let rotated =
                api_keys::rotate(&pool, user, workspace, ApiKeyId::new(key.record.id), AUDIT)
                    .await?;
            api_keys::revoke(
                &pool,
                user,
                workspace,
                ApiKeyId::new(rotated.record.id),
                AUDIT,
            )
            .await?;
            api_keys::list(&pool, user, workspace).await?;
            Ok::<_, ApiKeyError>(())
        });
    }
    barrier.wait().await;
    tokio::time::timeout(DEADLINE, async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query='SELECT id FROM workspaces WHERE id = $1 FOR UPDATE'")
                .fetch_one(&mut observer).await.unwrap();
            if waiting == i64::from(CONCURRENCY) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("all four transactions must occupy pool connections waiting for the same workspace lock");
    assert_eq!(pool.size(), CONCURRENCY);
    assert_eq!(pool.num_idle(), 0);
    blocking.rollback().await.unwrap();
    tokio::time::timeout(DEADLINE, async {
        while let Some(result) = tasks.join_next().await {
            result
                .expect("lifecycle task joins")
                .expect("lifecycle burst succeeds");
        }
    })
    .await
    .expect("saturated pool must make progress without a second authorization connection");
    let keys = api_keys::list(&pool, user, workspace).await.unwrap();
    assert_eq!(keys.len(), CONCURRENCY as usize * 2);
    assert!(keys.iter().all(|key| key.revoked_at.is_some()));
    let revoked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events WHERE event_type='api_key.revoked'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revoked, i64::from(CONCURRENCY) * 2);
    pool.close().await;
}
