use anyhow::{Context, Result};
use sqlx::ConnectOptions;
use std::{collections::HashMap, time::Duration};

#[tokio::test]
async fn actual_production_api_pool_refuses_unactivated_owner_credentials_before_serving()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 3).await
    else {
        return Ok(());
    };
    let mut runtime = deepref_config::RuntimeConfig::from_map(
        "role-startup-fixture",
        &HashMap::from([
            ("APP_ENV".to_owned(), "development".to_owned()),
            (
                "DATABASE_URL".to_owned(),
                pool.connect_options().to_url_lossy().to_string(),
            ),
        ]),
    )?;
    // Inject the environment to reach the production connection-role guard
    // without constructing external provider accounts or sending requests.
    runtime.environment = deepref_config::Environment::Production;
    let config = deepref_http_api::config::ApiConfig {
        runtime,
        documents: deepref_config::DocumentConfig::from_source(&|key| {
            (key == "APP_ENV").then(|| "development".into())
        })?,
        bind_addr: "127.0.0.1:0".parse()?,
        cors_allow_any: false,
        cors_origins: vec![],
    };
    let returned = tokio::time::timeout(
        Duration::from_secs(5),
        deepref_http_api::serve_with_shutdown(config, std::future::pending()),
    )
    .await
    .context("privileged production API reached serving instead of refusing its database role")?;
    let error = returned
        .err()
        .context("privileged production API unexpectedly started")?;
    anyhow::ensure!(format!("{error:#}").contains("database roles are not activated"));
    Ok(())
}
