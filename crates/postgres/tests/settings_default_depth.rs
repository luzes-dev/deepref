#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use deepref_postgres::MIGRATOR;
use sqlx::{
    Executor, PgPool, Postgres, migrate::Migrator, pool::PoolConnection, postgres::PgPoolOptions,
};
use uuid::Uuid;

/// Migration 0048 moves the workspace default citation depth from 2 to 1.
const DEPTH_MIGRATION_VERSION: i64 = 48;

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

/// A connection whose search path is a fresh schema, so a scenario starts from an empty database.
async fn isolated_connection(pool: &PgPool, schema: &str) -> PoolConnection<Postgres> {
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
    connection
}

async fn drop_schema(connection: &mut PoolConnection<Postgres>, schema: &str) {
    let drop_schema = format!("DROP SCHEMA \"{schema}\" CASCADE");
    connection
        .execute(sqlx::query(sqlx::AssertSqlSafe(drop_schema)))
        .await
        .expect("isolated schema cleanup");
}

/// Applies every migration before 0048, inserts the settings row described by `seed`, then applies
/// 0048 and returns the default depth the row ends up with.
async fn depth_after_migration(pool: &PgPool, seed: &str) -> i32 {
    let schema = format!("settings_depth_{}", Uuid::new_v4().simple());
    let mut connection = isolated_connection(pool, &schema).await;
    let before_depth_default = Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < DEPTH_MIGRATION_VERSION)
            .cloned()
            .collect(),
    );
    before_depth_default
        .run(&mut *connection)
        .await
        .expect("migrations before 0048 apply");
    connection
        .execute(sqlx::query(sqlx::AssertSqlSafe(seed.to_owned())))
        .await
        .expect("legacy settings row");
    MIGRATOR
        .run(&mut *connection)
        .await
        .expect("0048 applies to the legacy row");
    let depth: i32 = sqlx::query_scalar("SELECT default_max_depth FROM settings WHERE id = 1")
        .fetch_one(&mut *connection)
        .await
        .expect("settings row remains");
    drop_schema(&mut connection, &schema).await;
    depth
}

/// Every migration in a fresh schema leaves the depth defaults at 1 for settings, projects and
/// legacy ingestions, and a settings row created without values takes that default.
async fn assert_new_rows_default_to_one(pool: &PgPool) {
    let schema = format!("depth_defaults_{}", Uuid::new_v4().simple());
    let mut connection = isolated_connection(pool, &schema).await;
    MIGRATOR
        .run(&mut *connection)
        .await
        .expect("all migrations apply");

    let defaults: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT table_name::text || '.' || column_name::text, column_default::text
         FROM information_schema.columns
         WHERE table_schema = current_schema()
           AND (table_name, column_name) IN (
             ('settings', 'default_max_depth'),
             ('projects', 'default_max_depth'),
             ('ingestions', 'max_depth'))
         ORDER BY 1",
    )
    .fetch_all(&mut *connection)
    .await
    .expect("column defaults are readable");
    assert_eq!(
        defaults,
        vec![
            ("ingestions.max_depth".to_owned(), Some("1".to_owned())),
            (
                "projects.default_max_depth".to_owned(),
                Some("1".to_owned())
            ),
            (
                "settings.default_max_depth".to_owned(),
                Some("1".to_owned())
            ),
        ]
    );

    connection
        .execute("INSERT INTO settings (id) VALUES (1)")
        .await
        .expect("settings row with defaults");
    let settings_depth: i32 = sqlx::query_scalar("SELECT default_max_depth FROM settings")
        .fetch_one(&mut *connection)
        .await
        .expect("settings default");
    assert_eq!(settings_depth, 1);

    drop_schema(&mut connection, &schema).await;
}

/// The scenarios run one after another on purpose: the migrations install extensions (pg_trgm,
/// pgcrypto, vector) into the schema they run in, so two isolated migration runs at the same time
/// would remove each other's extensions.
#[tokio::test]
async fn depth_default_migration_moves_only_untouched_settings_and_new_rows_default_to_one() {
    let Some(pool) = database().await else {
        return;
    };

    let unchanged = depth_after_migration(
        &pool,
        "INSERT INTO settings (id, crossref_mailto, created_at, updated_at)
         VALUES (1, '', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .await;
    assert_eq!(unchanged, 1, "a row nobody has edited moves to depth 1");

    let edited = depth_after_migration(
        &pool,
        "INSERT INTO settings (id, crossref_mailto, created_at, updated_at)
         VALUES (1, '', '2026-01-01T00:00:00Z', '2026-02-01T00:00:00Z')",
    )
    .await;
    assert_eq!(
        edited, 2,
        "a row that was updated after creation keeps its depth"
    );

    let deliberate = depth_after_migration(
        &pool,
        "INSERT INTO settings (id, crossref_mailto, default_max_depth, created_at, updated_at)
         VALUES (1, '', 3, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .await;
    assert_eq!(deliberate, 3, "a deliberate non-default depth is kept");

    assert_new_rows_default_to_one(&pool).await;
}
