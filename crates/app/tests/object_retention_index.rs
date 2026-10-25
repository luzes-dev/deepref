#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use uuid::Uuid;

#[tokio::test]
async fn cleanup_retention_probe_uses_the_inverse_key_index_among_unrelated_references() {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 3).await
    else {
        return;
    };
    // A bounded metadata-only fixture makes a one-key retention lookup
    // selective without changing planner flags or using timing assertions.
    sqlx::query(
        "WITH objects AS (
        INSERT INTO object_lifecycle(storage_key)
        SELECT 'documents/' || gen_random_uuid()::text FROM generate_series(1,2000)
        RETURNING storage_key)
        INSERT INTO object_retained_references(reference_id,storage_key,expires_at)
        SELECT gen_random_uuid(),storage_key,now()+interval '1 day' FROM objects",
    )
    .execute(&pool)
    .await
    .unwrap();
    let target = format!("documents/{}", Uuid::new_v4());
    sqlx::query("INSERT INTO object_lifecycle(storage_key) VALUES($1)")
        .bind(&target)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO object_retained_references(reference_id,storage_key,expires_at) VALUES($1,$2,now()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(&target).execute(&pool).await.unwrap();
    sqlx::query("ANALYZE object_retained_references")
        .execute(&pool)
        .await
        .unwrap();
    let ready: bool = sqlx::query_scalar(
        "SELECT indisvalid AND indisready FROM pg_index
        WHERE indexrelid='object_retained_references_storage_expiry_idx'::regclass",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        ready,
        "the concurrent migration must leave a valid usable index"
    );
    let plan: serde_json::Value=sqlx::query_scalar("EXPLAIN (FORMAT JSON)
        SELECT max(expires_at) FROM object_retained_references WHERE storage_key=$1 AND expires_at>now()")
        .bind(&target).fetch_one(&pool).await.unwrap();
    assert!(
        plan.to_string()
            .contains("object_retained_references_storage_expiry_idx"),
        "the real cleanup retention predicate must use its inverse index: {plan}"
    );
    let retained: bool=sqlx::query_scalar("SELECT max(expires_at)>now() FROM object_retained_references WHERE storage_key=$1 AND expires_at>now()")
        .bind(&target).fetch_one(&pool).await.unwrap();
    assert!(retained);
}
