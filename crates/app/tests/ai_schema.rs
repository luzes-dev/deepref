#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use sqlx::{PgPool, Row};

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
}

#[tokio::test]
async fn ai_schema_requires_vector_state_and_typed_run_provenance() {
    let Some(pool) = database().await else { return };

    let extension: Option<String> =
        sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname='vector'")
            .fetch_optional(&pool)
            .await
            .expect("vector extension should be queryable");
    assert!(extension.is_some());

    let embedding_dimension: String = sqlx::query_scalar(
        "SELECT udt_name FROM information_schema.columns
         WHERE table_name='document_block_embeddings' AND column_name='embedding'",
    )
    .fetch_one(&pool)
    .await
    .expect("embedding column should exist");
    assert_eq!(embedding_dimension, "vector");

    let hnsw: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_indexes
         WHERE tablename='document_block_embeddings' AND indexname='document_block_embeddings_hnsw_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("embedding index should exist");
    assert_eq!(hnsw, 1);

    let route_columns = sqlx::query(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name='ai_runs' AND column_name IN
        ('profile','model_version','prompt_hash','schema_version','schema_hash','reuse_hash','evidence_refs')
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .expect("AI run columns should exist");
    let columns: Vec<String> = route_columns
        .into_iter()
        .map(|row| row.get("column_name"))
        .collect();
    assert_eq!(columns.len(), 7);

    let reuse_index: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_indexes
         WHERE tablename='ai_runs' AND indexname='ai_runs_reuse_completed_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("completed reuse index should exist");
    assert_eq!(reuse_index, 1);

    let proposal_columns: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM information_schema.columns
         WHERE table_name='ai_proposals' AND column_name IN
           ('entity_type','operation','model_run_id','authority_tier')",
    )
    .fetch_one(&pool)
    .await
    .expect("proposal columns should exist");
    assert_eq!(proposal_columns, 4);
}
