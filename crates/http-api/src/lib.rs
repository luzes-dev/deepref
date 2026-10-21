pub mod auth;
pub mod authz;
pub mod config;
pub mod error;
mod host_guard;
mod origin_guard;
pub mod routes;
pub mod state;

use sqlx::{PgPool, postgres::PgPoolOptions};
use std::future::Future;
use uuid::Uuid;

use crate::{config::ApiConfig, state::AppState};

pub async fn migrate(config: &ApiConfig) -> anyhow::Result<()> {
    let pool = database_pool(config).await?;
    deepref_app::migrate(&pool).await?;
    tracing::info!("database migrations completed");
    Ok(())
}

/// Operator step after `migrate` (docs/saas-program, handoff 03a): assigns the
/// migration workspace to every project that predates workspaces, one
/// committed batch at a time, then validates. Safe to interrupt and rerun.
pub async fn backfill_project_workspaces(config: &ApiConfig) -> anyhow::Result<()> {
    let pool = database_pool(config).await?;
    let report = deepref_app::backfill_project_workspaces(&pool, 500).await?;
    tracing::info!(
        batches = report.batches,
        moved = report.moved,
        users_marked_onboarded = report.users_marked_onboarded,
        total_projects = report.total_projects,
        "project workspace backfill completed and validated"
    );
    Ok(())
}

/// Apply a trusted operator mapping from a pre-canonical actor label to a
/// DeepRef user. The app store commits the mapping and its audit row together.
pub async fn legacy_actor_map(
    config: &ApiConfig,
    actor_kind: &str,
    actor_id: &str,
    user_id: Uuid,
    operator_user_id: Uuid,
    source: &str,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let decision = deepref_app::map_legacy_actor(
        &pool,
        actor_kind,
        actor_id,
        deepref_domain::UserId::new(user_id),
        deepref_domain::UserId::new(operator_user_id),
        source,
    )
    .await?;
    Ok(serde_json::to_string_pretty(&decision)?)
}

/// Record a trusted operator confirmation that a legacy actor label does not
/// belong to a particular user.
pub async fn legacy_actor_deny(
    config: &ApiConfig,
    actor_kind: &str,
    actor_id: &str,
    user_id: Uuid,
    operator_user_id: Uuid,
    source: &str,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let decision = deepref_app::record_legacy_actor_non_identity(
        &pool,
        actor_kind,
        actor_id,
        deepref_domain::UserId::new(user_id),
        deepref_domain::UserId::new(operator_user_id),
        source,
    )
    .await?;
    Ok(serde_json::to_string_pretty(&decision)?)
}

/// List all operator-confirmed mapping and non-identity decisions.
pub async fn legacy_actor_list(config: &ApiConfig) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let decisions = deepref_app::list_legacy_actor_decisions(&pool).await?;
    Ok(serde_json::to_string_pretty(&decisions)?)
}

/// Filters for `deepref-server audit query` (11 Part B). All optional and
/// combined with AND.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditQueryParams {
    pub workspace_id: Option<Uuid>,
    pub principal_user_id: Option<Uuid>,
    pub target_user_id: Option<Uuid>,
    pub event_type: Option<String>,
    /// RFC 3339 lower bound, inclusive.
    pub since: Option<String>,
    /// RFC 3339 upper bound, inclusive.
    pub until: Option<String>,
    /// Maximum rows; zero means the default.
    pub limit: i64,
}

fn parse_time(
    name: &str,
    value: Option<&str>,
) -> anyhow::Result<Option<chrono::DateTime<chrono::Utc>>> {
    value
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .map(|time| time.with_timezone(&chrono::Utc))
                .map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))
        })
        .transpose()
}

/// Incident-response query over the administrative audit, one JSON object per
/// line.
pub async fn audit_query(config: &ApiConfig, params: AuditQueryParams) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let query = deepref_app::audit_admin::AuditQuery {
        workspace_id: params.workspace_id,
        principal_user_id: params.principal_user_id,
        target_user_id: params.target_user_id,
        event_type: params.event_type,
        since: parse_time("--since", params.since.as_deref())?,
        until: parse_time("--until", params.until.as_deref())?,
        limit: params.limit,
    };
    let rows = deepref_app::audit_admin::query_admin_audit(&pool, &query).await?;
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        lines.push(serde_json::to_string(&row)?);
    }
    Ok(lines.join("\n"))
}

/// Retention purge of the administrative audit: 6 months for administrative
/// and security rows, 5 years for the deletion-proof completion record
/// (BUSINESS_DECISIONS). With `dry_run` nothing is deleted.
pub async fn audit_purge(config: &ApiConfig, dry_run: bool) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let report = deepref_app::audit_admin::purge_admin_audit(&pool, dry_run, None).await?;
    Ok(serde_json::to_string_pretty(&report)?)
}

/// Completes every account deletion whose 30-day window has passed, and
/// re-applies deletions a restore undid (11, BUSINESS_DECISIONS).
pub async fn complete_account_deletions(
    config: &ApiConfig,
    deletion_digest: &deepref_config::DeletionDigestConfig,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let report = deepref_app::account_deletion::complete_account_deletions(
        &pool,
        deletion_digest.expose_secret(),
        deepref_app::identity::AuditRequest::default(),
    )
    .await?;
    Ok(serde_json::to_string_pretty(&report)?)
}

pub async fn serve(config: ApiConfig) -> anyhow::Result<()> {
    serve_with_shutdown(config, shutdown_signal()).await
}

pub async fn serve_with_shutdown(
    config: ApiConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    deepref_application::validate_shipped_appraisal_definitions()
        .map_err(|error| anyhow::anyhow!("invalid shipped appraisal definition: {error}"))?;
    let pool = database_pool(&config).await?;
    deepref_app::identity::ensure_development_principal(
        &pool,
        config.runtime.development_principal_enabled(),
    )
    .await?;

    let document_store = deepref_documents::DocumentStore::from_config(&config.documents)?;
    anyhow::ensure!(
        deepref_app::configure_workflow_runtime(config.runtime.workflow.niche_nodes_enabled),
        "API and worker workflow feature configuration must agree"
    );
    let mut state = AppState::new(pool.clone())
        .with_document_store(document_store)
        .with_smtp(config.runtime.smtp.clone())
        .with_workflow_config(config.runtime.workflow.clone())
        .with_integrations(config.runtime.integrations.clone());
    if config.runtime.integrations.contact_email.is_empty() {
        tracing::warn!(
            "DEEPREF_CONTACT_EMAIL is not set; DOI imports, PubMed imports and open-access lookups will be refused"
        );
    }
    tracing::info!(
        metadata_providers = %config.runtime.integrations.metadata_chain_label(),
        "metadata provider chain configured"
    );
    let ai_config = &config.runtime.ai;
    for notice in ai_config.deprecations() {
        tracing::warn!("{notice}");
    }
    let provider = ai_config.provider;
    match ai_config.api_key() {
        Some(api_key) => {
            let gateway = deepref_ai::build_metered_provider(
                provider.id(),
                &ai_config.base_url,
                api_key,
                std::sync::Arc::new(deepref_app::PostgresUsageLedger::new(&pool)),
                price_book(ai_config),
            )?;
            let routes = deepref_app::ensure_default_model_routes(
                &pool,
                provider.id(),
                &ai_config.default_model,
            )
            .await?;
            tracing::info!(
                provider = provider.id(),
                model = %ai_config.default_model,
                routes_created = routes.created,
                routes_repointed = routes.repointed,
                "AI provider configured"
            );
            state =
                state.with_ai_provider(gateway, provider.label(), ai_config.default_model.clone());
        }
        None => tracing::warn!(
            provider = provider.id(),
            "{} is not set; AI features are disabled",
            provider.api_key_variable()
        ),
    }
    if config.runtime.auth_enabled {
        let workos =
            config.runtime.workos.as_ref().ok_or_else(|| {
                anyhow::anyhow!("DEEPREF_AUTH_ENABLED requires the WorkOS settings")
            })?;
        state = state.with_auth(auth::AuthRuntime::new(
            std::sync::Arc::new(deepref_integrations::WorkOsIdentityProvider::new(workos)),
            &config.runtime.public_origin,
            workos.cookie_password(),
        ));
        tracing::info!("authentication enabled");
    } else {
        tracing::warn!("DEEPREF_AUTH_ENABLED is off; the API serves requests without a session");
    }
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "API listening");
    axum::serve(listener, routes::router(state, &config))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// The built-in price table plus the `DEEPREF_AI_MODEL_PRICES` overrides.
fn price_book(config: &deepref_config::AiProviderConfig) -> deepref_ai::PriceBook {
    deepref_ai::PriceBook::new(config.price_overrides.iter().map(|price| {
        (
            price.provider.clone(),
            price.model.clone(),
            deepref_ai::ModelPrice {
                input_micros_per_million_tokens: price.input_micros_per_million_tokens,
                output_micros_per_million_tokens: price.output_micros_per_million_tokens,
            },
        )
    }))
}

async fn database_pool(config: &ApiConfig) -> anyhow::Result<PgPool> {
    let database = &config.runtime.database;
    Ok(PgPoolOptions::new()
        .min_connections(database.pool_min)
        .max_connections(database.pool_max)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(Some(database.idle_timeout))
        .max_lifetime(Some(database.max_lifetime))
        .connect(database.url())
        .await?)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = async {
                if let Some(stream) = terminate.as_mut() {
                    stream.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
