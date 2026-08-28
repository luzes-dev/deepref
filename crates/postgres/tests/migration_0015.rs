use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection, Row};
use uuid::Uuid;

const MIGRATIONS_THROUGH_PR9: [&str; 14] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_metrics.sql"),
    include_str!("../migrations/0003_outbox_claims.sql"),
    include_str!("../migrations/0004_ingestion_durability.sql"),
    include_str!("../migrations/0005_domain_projection.sql"),
    include_str!("../migrations/0006_evidence_workspace.sql"),
    include_str!("../migrations/0007_evidence_identity.sql"),
    include_str!("../migrations/0008_infrastructure_collapse.sql"),
    include_str!("../migrations/0009_acquisition_runs.sql"),
    include_str!("../migrations/0010_deduplication.sql"),
    include_str!("../migrations/0011_protocol_versions.sql"),
    include_str!("../migrations/0012_title_abstract_screening.sql"),
    include_str!("../migrations/0013_documents_full_text.sql"),
    include_str!("../migrations/0014_studies_appraisals.sql"),
];

#[tokio::test]
async fn migration_0015_adds_reason_freshness_without_reading_missing_created_at() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        return;
    };
    let mut connection = PgConnection::connect(&url)
        .await
        .expect("database connects");
    let schema = format!("pr10_migration_{}", Uuid::new_v4().simple());
    connection
        .execute(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .await
        .expect("isolated schema creates");
    connection
        .execute(AssertSqlSafe(format!("SET search_path TO {schema},public")))
        .await
        .expect("search path changes");

    for migration in MIGRATIONS_THROUGH_PR9 {
        connection
            .execute(migration)
            .await
            .expect("prior migration applies");
    }
    connection
        .execute(include_str!("../migrations/0015_prisma_freshness.sql"))
        .await
        .expect("PR10 freshness migration applies");

    let columns = sqlx::query(
        "SELECT column_name, is_nullable FROM information_schema.columns
         WHERE table_schema=current_schema() AND table_name='exclusion_reasons'
           AND column_name='updated_at'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("freshness column exists");
    assert_eq!(columns.get::<String, _>("column_name"), "updated_at");
    assert_eq!(columns.get::<String, _>("is_nullable"), "NO");

    let trigger_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_trigger
         WHERE tgrelid='exclusion_reasons'::regclass
           AND tgname='exclusion_reasons_touch_updated_at'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("freshness trigger is installed");
    assert_eq!(trigger_count, 1);

    connection
        .execute(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .await
        .expect("isolated schema drops");
}
