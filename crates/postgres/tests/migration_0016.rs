use deepref_postgres::{MIGRATOR, migrate};
use sqlx::{Executor, PgPool, Row, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("DATABASE_URL database must be reachable"),
    )
}

#[tokio::test]
async fn ai_foundation_evolves_representative_legacy_rows_without_loss() {
    let Some(pool) = database().await else { return };
    let schema = format!("ai_legacy_{}", Uuid::new_v4().simple());
    let mut connection = pool.acquire().await.expect("connection");
    let create_schema = format!("CREATE SCHEMA \"{schema}\"");
    connection
        .execute(sqlx::query(sqlx::AssertSqlSafe(create_schema)))
        .await
        .expect("isolated schema");
    let set_search_path = format!("SET search_path TO \"{schema}\", public");
    connection
        .execute(sqlx::query(sqlx::AssertSqlSafe(set_search_path)))
        .await
        .expect("isolated search path");
    let through_0015 = Migrator::with_migrations(MIGRATOR.iter().take(15).cloned().collect());
    through_0015
        .run(&mut *connection)
        .await
        .expect("legacy migrations apply");
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    connection
        .execute(sqlx::query("INSERT INTO projects(id,name) VALUES ($1,'legacy')").bind(project_id))
        .await
        .expect("legacy project");
    connection
        .execute(sqlx::query(
            "INSERT INTO ai_runs(id,project_id,task_kind,provider,model,prompt_version,input_hash,output,status)
             VALUES ($1,$2,'study_design_classification','legacy-provider','legacy-model','legacy.v1',$3,'{}','completed')",
        ).bind(run_id).bind(project_id).bind("a".repeat(64)))
        .await
        .expect("legacy run");
    connection
        .execute(sqlx::query(
            "INSERT INTO ai_proposals(id,project_id,ai_run_id,proposal_type,payload,status,decided_by,decided_at)
             VALUES ($1,$2,$3,'classify','{}','accepted','legacy-user',now())",
        ).bind(proposal_id).bind(project_id).bind(run_id))
        .await
        .expect("legacy proposal");
    let through_0016 = Migrator::with_migrations(MIGRATOR.iter().take(16).cloned().collect());
    through_0016
        .run(&mut *connection)
        .await
        .expect("AI migration preserves legacy rows");
    let run_status: String = sqlx::query_scalar("SELECT status FROM ai_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(&mut *connection)
        .await
        .expect("legacy run remains");
    assert_eq!(run_status, "completed");
    let (entity_type, authority, model_run_id): (String, String, Uuid) = sqlx::query_as(
        "SELECT entity_type,authority_tier,model_run_id FROM ai_proposals WHERE id=$1",
    )
    .bind(proposal_id)
    .fetch_one(&mut *connection)
    .await
    .expect("legacy proposal backfill");
    assert_eq!(entity_type, "classify");
    assert_eq!(authority, "workflow_suggestion");
    assert_eq!(model_run_id, run_id);
    let drop_schema = format!("DROP SCHEMA \"{schema}\" CASCADE");
    connection
        .execute(sqlx::query(sqlx::AssertSqlSafe(drop_schema)))
        .await
        .expect("isolated schema cleanup");
}

#[tokio::test]
async fn ai_foundation_migration_requires_and_installs_vector_state() {
    let Some(pool) = database().await else { return };
    migrate(&pool).await.expect("all migrations should apply");

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
