#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Retention purge and the append-only gate (handoff 11 Part B,
//! BUSINESS_DECISIONS: 6 months of administrative/security audit, 5 years for
//! the proof that a deletion request was fulfilled).

use chrono::{DateTime, Utc};
use deepref_app::audit_admin::{PurgeReport, purge_admin_audit};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

async fn make_user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, $2, 'purge@example.org', 'active')",
    )
    .bind(id)
    .bind(format!("user_{id}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Inserts one audit row at an explicit time. Only INSERT can set
/// `occurred_at`: the append-only trigger refuses every UPDATE.
async fn insert_event(pool: &PgPool, event: &str, at: DateTime<Utc>, target: Option<Uuid>) -> Uuid {
    let metadata = match event {
        "audit.purged" => serde_json::json!({ "rows_purged": 0 }),
        _ => serde_json::json!({}),
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id, metadata, occurred_at)
         VALUES ($1, $2, 'system', NULL, $3, $4, $5)",
    )
    .bind(id)
    .bind(event)
    .bind(target)
    .bind(metadata)
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn months_ago(months: i64) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::days(30 * months)
}

#[tokio::test]
async fn the_purge_removes_only_rows_past_their_retention_cutoff() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool).await;
    let old = insert_event(&pool, "auth.login", months_ago(7), Some(user)).await;
    let young = insert_event(&pool, "auth.login", Utc::now(), Some(user)).await;
    // The deletion-proof completion record is kept five years (7 months is
    // still inside its window).
    let proof_young = insert_event(
        &pool,
        "account.deletion_completed",
        months_ago(7),
        Some(user),
    )
    .await;
    let proof_old = insert_event(
        &pool,
        "account.deletion_completed",
        months_ago(66),
        Some(user),
    )
    .await;

    // The fulfilled-deletion log follows the same five-year retention.
    let logged_old = make_user(&pool).await;
    let logged_kept = make_user(&pool).await;
    sqlx::query("INSERT INTO fulfilled_account_deletions (user_id, completed_at) VALUES ($1, $2)")
        .bind(logged_old)
        .bind(months_ago(66))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fulfilled_account_deletions (user_id, completed_at) VALUES ($1, $2)")
        .bind(logged_kept)
        .bind(months_ago(7))
        .execute(&pool)
        .await
        .unwrap();

    // A dry run reports and changes nothing.
    let dry = purge_admin_audit(&pool, true, None).await.unwrap();
    assert_eq!(
        dry,
        PurgeReport {
            dry_run: true,
            batch_size: 1_000,
            batches: 0,
            audit_rows_purged: 2,
            fulfilled_deletion_rows_purged: 1,
            session_rows_purged: 0,
        }
    );
    assert!(exists(&pool, old).await, "a dry run deletes nothing");

    // One row per batch proves the delete is bounded.
    let report = purge_admin_audit(&pool, false, Some(1)).await.unwrap();
    assert_eq!(report.audit_rows_purged, 2);
    assert_eq!(report.fulfilled_deletion_rows_purged, 1);
    assert_eq!(
        report.batches, 3,
        "two batches of one row plus the empty one"
    );

    assert!(
        !exists(&pool, old).await,
        "seven months old is past retention"
    );
    assert!(
        !exists(&pool, proof_old).await,
        "five years old is past the proof window"
    );
    assert!(exists(&pool, young).await, "young rows are kept");
    assert!(
        exists(&pool, proof_young).await,
        "the deletion-proof record is kept for five years"
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fulfilled_account_deletions WHERE user_id = $1"
        )
        .bind(logged_old)
        .fetch_one(&pool)
        .await
        .unwrap()
            == 0
    );
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fulfilled_account_deletions WHERE user_id = $1"
        )
        .bind(logged_kept)
        .fetch_one(&pool)
        .await
        .unwrap()
            == 1
    );

    // The purge is itself an administrative action and says so.
    let purged = sqlx::query(
        "SELECT metadata->>'rows_purged' AS rows FROM admin_audit_events
         WHERE event_type = 'audit.purged' ORDER BY occurred_at DESC",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purged.get::<String, _>("rows"), "2");
}

async fn exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM admin_audit_events WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
        == 1
}

#[tokio::test]
async fn the_append_only_trigger_still_blocks_every_other_delete() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool).await;
    let old = insert_event(&pool, "auth.login", months_ago(7), Some(user)).await;

    // Without the purge gate even a row past retention cannot be deleted.
    let refused = sqlx::query("DELETE FROM admin_audit_events WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");

    // UPDATE and TRUNCATE are always refused.
    let refused = sqlx::query("UPDATE admin_audit_events SET metadata = '{}' WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");
    let refused = sqlx::query("TRUNCATE admin_audit_events")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");

    // The gate is transaction-local: it does not survive its transaction.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL deepref.audit_purge = 'on'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM admin_audit_events WHERE id = $1")
        .bind(old)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let young = insert_event(&pool, "auth.login", Utc::now(), Some(user)).await;
    let refused = sqlx::query("DELETE FROM admin_audit_events WHERE id = $1")
        .bind(young)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");
}

#[tokio::test]
async fn the_purge_gate_cannot_reach_young_or_deletion_proof_rows() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool).await;
    let young = insert_event(&pool, "auth.login", months_ago(1), Some(user)).await;
    let proof = insert_event(
        &pool,
        "account.deletion_completed",
        months_ago(7),
        Some(user),
    )
    .await;
    let old = insert_event(&pool, "auth.login", months_ago(7), Some(user)).await;

    for id in [young, proof] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL deepref.audit_purge = 'on'")
            .execute(&mut *tx)
            .await
            .unwrap();
        let refused = sqlx::query("DELETE FROM admin_audit_events WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("append-only"), "{refused}");
        tx.rollback().await.unwrap();
    }

    // The one combination the gate allows: the purge key and a row past its
    // cutoff.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL deepref.audit_purge = 'on'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM admin_audit_events WHERE id = $1")
        .bind(old)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!exists(&pool, old).await);
    assert!(exists(&pool, young).await);
    assert!(exists(&pool, proof).await);
}

/// One session row, inserted at an explicit age so the retention window can be
/// exercised without waiting.
async fn make_session(pool: &PgPool, user: Uuid, age_days: i64, revoked: bool) -> Uuid {
    let id = Uuid::new_v4();
    let mut hash = [0u8; 32];
    hash[0..16].copy_from_slice(&id.to_bytes_le());
    sqlx::query(
        "INSERT INTO user_sessions
           (id, user_id, session_token_hash, sealed_provider_session, session_ref,
            created_at, expires_at, revoked_at)
         VALUES ($1, $2, $3, 'sealed test provider session', $3,
                 now() - make_interval(days => $4),
                 now() - make_interval(days => $4) + interval '1 day',
                 CASE WHEN $5 THEN now() - make_interval(days => $4) ELSE NULL END)",
    )
    .bind(id)
    .bind(user)
    .bind(&hash[..])
    .bind(age_days as i32)
    .bind(revoked)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn the_purge_also_removes_sessions_thirty_days_past_expiry_or_revocation() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool).await;
    let expired_old = make_session(&pool, user, 40, false).await;
    let revoked_old = make_session(&pool, user, 40, true).await;
    let expired_recent = make_session(&pool, user, 10, false).await;
    let revoked_recent = make_session(&pool, user, 10, true).await;
    sqlx::query(
        "INSERT INTO user_sessions
           (id, user_id, session_token_hash, sealed_provider_session, session_ref, expires_at)
         VALUES ($1, $2, $3, 'sealed test provider session', $3, now() + interval '1 hour')",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(&vec![7u8; 32][..])
    .execute(&pool)
    .await
    .unwrap();

    let dry = purge_admin_audit(&pool, true, None).await.unwrap();
    assert_eq!(dry.session_rows_purged, 2, "only the two old rows are due");
    assert!(
        session_exists(&pool, expired_old).await,
        "a dry run deletes nothing"
    );

    let report = purge_admin_audit(&pool, false, None).await.unwrap();
    assert_eq!(report.session_rows_purged, 2);
    assert!(!session_exists(&pool, expired_old).await);
    assert!(!session_exists(&pool, revoked_old).await);
    assert!(
        session_exists(&pool, expired_recent).await,
        "ten days after expiry is still inside retention"
    );
    assert!(
        session_exists(&pool, revoked_recent).await,
        "ten days after revocation is still inside retention"
    );
}

async fn session_exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_sessions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
        == 1
}
