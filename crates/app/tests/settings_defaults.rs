#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use sqlx::{Executor, PgPool};

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
}

/// Fresh databases keep active settings/project depth defaults at 1,
/// and a settings row created without values takes that default.
async fn assert_new_rows_default_to_one(pool: &PgPool) {
    let mut connection = pool.acquire().await.expect("connection");

    let defaults: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT table_name::text || '.' || column_name::text, column_default::text
         FROM information_schema.columns
         WHERE table_schema = current_schema()
           AND (table_name, column_name) IN (
             ('settings', 'default_max_depth'),
             ('projects', 'default_max_depth'))
         ORDER BY 1",
    )
    .fetch_all(&mut *connection)
    .await
    .expect("column defaults are readable");
    assert_eq!(
        defaults,
        vec![
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
}

#[tokio::test]
async fn fresh_settings_and_project_depth_defaults_are_one() {
    let Some(pool) = database().await else { return };
    assert_new_rows_default_to_one(&pool).await;
}
