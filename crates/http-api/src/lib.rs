pub mod auth;
pub mod authz;
pub mod config;
pub mod error;
mod host_guard;
mod origin_guard;
pub mod routes;
pub mod state;

use sqlx::{PgPool, Row, postgres::PgPoolOptions};
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

/// Read-only reconciliation report for AI reservations. The durable
/// workspace-scoped worker owns settlement; this operator command reports
/// mismatches and never modifies or releases a reservation.
pub async fn ai_usage_reconcile(
    config: &ApiConfig,
    workspace_id: Option<Uuid>,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let rows = sqlx::query(
        "SELECT w.id AS workspace_id,
                COUNT(r.id) FILTER (WHERE r.state = 'open') AS open,
                COUNT(r.id) FILTER (WHERE r.state = 'observed') AS observed,
                COUNT(r.id) FILTER (WHERE r.state = 'unresolved') AS unresolved,
                COUNT(r.id) FILTER (WHERE r.state IN ('open','observed','unresolved')
                  AND r.job_id IS NOT NULL
                  AND (j.id IS NULL
                    OR (j.claim_seq <> r.attempt_fence
                        AND (j.state <> 'running' OR j.lease_owner IS NOT NULL))
                    OR (j.state <> 'running' AND j.lease_owner IS NULL AND j.leased_until IS NULL))) AS stale_fenced
         FROM workspaces w
         LEFT JOIN ai_usage_reservations r ON r.workspace_id = w.id
         LEFT JOIN jobs j ON j.id = r.job_id
         WHERE ($1::uuid IS NULL OR w.id = $1)
         GROUP BY w.id ORDER BY w.id",
    )
    .bind(workspace_id)
    .fetch_all(&pool)
    .await?;
    let reports = rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "workspace_id": row.get::<Uuid, _>("workspace_id"),
                "open": row.get::<i64, _>("open"),
                "observed": row.get::<i64, _>("observed"),
                "unresolved": row.get::<i64, _>("unresolved"),
                "stale_fenced": row.get::<i64, _>("stale_fenced"),
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&reports)?)
}

pub async fn ai_usage_waive_overage(
    config: &ApiConfig,
    workspace_id: Uuid,
    operator_user_id: Uuid,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    deepref_app::waive_ai_usage_overage(
        &pool,
        deepref_domain::WorkspaceId::new(workspace_id),
        deepref_domain::UserId::new(operator_user_id),
    )
    .await?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "workspace_id": workspace_id,
        "waived": true,
    }))?)
}

pub async fn ai_usage_set_cap(
    config: &ApiConfig,
    scope_kind: &str,
    scope_id: Option<Uuid>,
    limit_micros: i64,
    operator_user_id: Uuid,
) -> anyhow::Result<String> {
    let scope = match (scope_kind, scope_id) {
        ("platform", None) => deepref_app::AiUsageLimitScope::Platform,
        ("workspace", Some(id)) => {
            deepref_app::AiUsageLimitScope::Workspace(deepref_domain::WorkspaceId::new(id))
        }
        ("project", Some(id)) => {
            deepref_app::AiUsageLimitScope::Project(deepref_domain::ProjectId::new(id))
        }
        _ => anyhow::bail!("scope and scope id do not match"),
    };
    let pool = database_pool(config).await?;
    deepref_app::set_ai_usage_cap(
        &pool,
        scope,
        limit_micros,
        deepref_domain::UserId::new(operator_user_id),
    )
    .await?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "scope_kind": scope_kind,
        "scope_id": scope_id,
        "limit_micros": limit_micros,
        "changed": true,
    }))?)
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
    deepref_ai::PriceBook::new(config.model_price_table().into_iter().map(|price| {
        (
            price.provider,
            price.model,
            deepref_ai::ModelPrice {
                input_micros_per_million_tokens: price.input_micros_per_million_tokens,
                output_micros_per_million_tokens: price.output_micros_per_million_tokens,
                reasoning_micros_per_million_tokens: price.reasoning_micros_per_million_tokens,
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
