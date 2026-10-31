pub mod auth;
pub mod authz;
pub mod config;
pub mod error;
mod host_guard;
mod metrics;
mod origin_guard;
mod rate_limits;
pub mod routes;
pub mod state;

use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::future::Future;
use uuid::Uuid;

use crate::{config::ApiConfig, state::AppState};

/// Facts one `ai-usage export-replay` invocation inspects without `--limit`.
const DEFAULT_USAGE_EXPORT_REPLAY_LIMIT: i64 = 10_000;

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

/// Read-only reconciliation report for AI reservations. The durable
/// workspace-scoped worker owns settlement; this operator command reports
/// mismatches and never modifies or releases a reservation.
pub async fn ai_usage_reconcile(
    config: &ApiConfig,
    workspace_id: Option<Uuid>,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let rows = sqlx::query(
        "WITH workspace_ids AS (
           SELECT id FROM workspaces
           UNION
           SELECT workspace_id FROM ai_usage_reservations
           WHERE state IN ('open', 'observed', 'unresolved')
         )
         SELECT w.id AS workspace_id,
                COUNT(r.id) FILTER (WHERE r.state = 'open') AS open,
                COUNT(r.id) FILTER (WHERE r.state = 'observed') AS observed,
                COUNT(r.id) FILTER (WHERE r.state = 'unresolved') AS unresolved,
                COUNT(r.id) FILTER (WHERE r.state IN ('open','observed','unresolved')
                  AND r.job_id IS NOT NULL
                  AND (j.id IS NULL
                    OR (j.claim_seq <> r.attempt_fence
                        AND (j.state <> 'running' OR j.lease_owner IS NOT NULL))
                    OR (j.state <> 'running' AND j.lease_owner IS NULL AND j.leased_until IS NULL))) AS stale_fenced
         FROM workspace_ids w
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

pub async fn activate_tenant_object_keys(
    config: &ApiConfig,
    attestation: &str,
) -> anyhow::Result<()> {
    let pool = database_pool(config).await?;
    deepref_app::object_lifecycle::activate_tenant_object_keys(&pool, attestation).await
}

pub async fn reconcile_object_orphans(
    config: &ApiConfig,
    attestation: &str,
) -> anyhow::Result<u64> {
    let pool = database_pool(config).await?;
    let store = deepref_documents::DocumentStore::from_config(&config.documents)?;
    deepref_app::object_lifecycle::reconcile_legacy_orphans(&pool, &store, 10_000, attestation)
        .await
}

/// Explicit object-storage maintenance cutover; never run by migration startup.
pub async fn workflow_storage_cutover(
    config: &ApiConfig,
    contract: bool,
    attestation: &str,
) -> anyhow::Result<u64> {
    let pool = database_pool(config).await?;
    let store = deepref_documents::DocumentStore::from_config(&config.documents)?;
    let target = if contract {
        deepref_app::WorkflowFileCutover::Contract
    } else {
        deepref_app::WorkflowFileCutover::ObjectOnly
    };
    deepref_app::cutover_workflow_file_storage(&pool, &store, target, attestation).await
}

pub async fn ai_usage_backfill(config: &ApiConfig, batch_size: usize) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let summary = deepref_app::backfill_ai_usage_legacy(&pool, batch_size).await?;
    Ok(serde_json::to_string_pretty(&summary)?)
}

/// Emit one bounded, resumable batch of usage fact export events from retained
/// usage facts (handoff 09 slice B). Idempotent: rerunning it re-emits nothing,
/// and a fact settled while the export switch was off is emitted now.
pub async fn ai_usage_export_replay(
    config: &ApiConfig,
    since: Option<chrono::DateTime<chrono::Utc>>,
    limit: Option<i64>,
) -> anyhow::Result<String> {
    let settings = &config.runtime.usage_export;
    let limit = limit.unwrap_or(DEFAULT_USAGE_EXPORT_REPLAY_LIMIT);
    anyhow::ensure!(limit > 0, "limit must be positive");
    let pool = database_pool(config).await?;
    let counters = deepref_application::usage_export::UsageExportCounters::default();
    let summary = deepref_app::replay_usage_facts(
        &pool,
        &counters,
        deepref_app::UsageExportReplayParams {
            enabled: settings.enabled,
            since,
            limit,
        },
    )
    .await?;
    tracing::info!(
        enabled = summary.enabled,
        facts_scanned = summary.facts_scanned,
        facts_emitted = summary.facts_emitted,
        corrections_emitted = summary.corrections_emitted,
        complete = summary.complete,
        "AI usage export replay finished"
    );
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "summary": summary,
        "counters": counters.snapshot(),
    }))?)
}

/// Activate the usage export rollout gate after checking the live worker
/// fleet decodes the replay job kind and the N-1 quiet window.
pub async fn ai_usage_enable_export(
    config: &ApiConfig,
    operator_user_id: Uuid,
    quiet_window_seconds: u64,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    deepref_app::enable_ai_usage_export(
        &pool,
        deepref_domain::UserId::new(operator_user_id),
        quiet_window_seconds,
    )
    .await?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "ai_usage_export_activated": true,
        "quiet_window_seconds": quiet_window_seconds,
    }))?)
}

/// Deactivate the usage export rollout gate and park queued replay jobs, so a
/// rollback can never expose the job kind to a binary that would dead-letter
/// it. Rollback requires this command first.
pub async fn ai_usage_disable_export(
    config: &ApiConfig,
    operator_user_id: Uuid,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    let parked_jobs =
        deepref_app::disable_ai_usage_export(&pool, deepref_domain::UserId::new(operator_user_id))
            .await?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "ai_usage_export_activated": false,
        "parked_jobs": parked_jobs,
    }))?)
}

pub async fn ai_usage_enable_hard_caps(
    config: &ApiConfig,
    operator_user_id: Uuid,
    quiet_window_seconds: u64,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    deepref_app::enable_ai_hard_caps(
        &pool,
        deepref_domain::UserId::new(operator_user_id),
        quiet_window_seconds,
    )
    .await?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "ai_hard_caps_enforced": true,
        "quiet_window_seconds": quiet_window_seconds,
    }))?)
}

pub async fn ai_usage_disable_hard_caps(
    config: &ApiConfig,
    operator_user_id: Uuid,
) -> anyhow::Result<String> {
    let pool = database_pool(config).await?;
    deepref_app::disable_ai_hard_caps(&pool, deepref_domain::UserId::new(operator_user_id)).await?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "ai_hard_caps_enforced": false,
    }))?)
}

pub async fn ai_usage_resolve(
    config: &ApiConfig,
    reservation_id: Uuid,
    operator_user_id: Uuid,
    actual_cost_micros: Option<i64>,
    reason: Option<String>,
) -> anyhow::Result<String> {
    let resolution = match (actual_cost_micros, reason) {
        (Some(actual_cost_micros), None) => {
            deepref_app::AiUsageResolution::Settle { actual_cost_micros }
        }
        (None, Some(reason)) => deepref_app::AiUsageResolution::Void { reason },
        _ => anyhow::bail!("provide exactly one of --settle or --void --reason"),
    };
    let pool = database_pool(config).await?;
    deepref_app::resolve_ai_usage_reservation(
        &pool,
        reservation_id,
        deepref_domain::UserId::new(operator_user_id),
        resolution.clone(),
    )
    .await?;
    let result = match resolution {
        deepref_app::AiUsageResolution::Settle { actual_cost_micros } => serde_json::json!({
            "reservation_id": reservation_id,
            "resolution": "settle",
            "actual_cost_micros": actual_cost_micros,
        }),
        deepref_app::AiUsageResolution::Void { reason } => serde_json::json!({
            "reservation_id": reservation_id,
            "resolution": "void",
            "reason": reason,
        }),
    };
    Ok(serde_json::to_string_pretty(&result)?)
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
    if let Some(billing) = config.runtime.dodo.clone() {
        let provider = deepref_integrations::dodo::DodoBilling::new(billing)?;
        state.billing = Some(deepref_app::billing::BillingService::new(
            pool.clone(),
            std::sync::Arc::new(provider),
        ));
    }

    let rate_observer =
        std::sync::Arc::new(deepref_telemetry::operational::OperationalMetrics::default());
    state = state.with_rate_limiter(std::sync::Arc::new(
        deepref_app::rate_limits::PostgresRateLimiter::new(pool.clone())
            .with_observer(rate_observer.clone()),
    ));
    if let Some(valkey) = &config.runtime.valkey {
        state.project_count_cache =
            deepref_integrations::valkey_project_counts::ValkeyProjectCountCache::from_config(
                valkey,
            )
            .ok()
            .map(|cache| {
                std::sync::Arc::new(
                    deepref_telemetry::project_count_cache::ObservedProjectCountCache::new(
                        std::sync::Arc::new(cache),
                        deepref_telemetry::project_count_cache::ProjectCountMetrics::default(),
                    ),
                )
                    as std::sync::Arc<
                        dyn deepref_application::project_count_cache::ProjectCountCache,
                    >
            });
        match deepref_integrations::valkey::ValkeyRateAccelerator::from_config(valkey) {
            Ok(accelerator) => {
                state = state.with_rate_limiter(std::sync::Arc::new(
                    deepref_app::rate_limits::PostgresRateLimiter::new(pool.clone())
                        .with_accelerator(std::sync::Arc::new(accelerator))
                        .with_observer(rate_observer),
                ));
            }
            Err(_) => tracing::warn!(
                "Valkey configuration cannot initialize acceleration; using shared PostgreSQL ceiling"
            ),
        }
    }
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
            let canonical: std::sync::Arc<dyn deepref_ai::AiConcurrencyAdmission> =
                std::sync::Arc::new(deepref_app::PostgresAiConcurrencyAdmission::new(&pool));
            let admission = match config.runtime.valkey.as_ref().and_then(|config| {
                deepref_integrations::valkey_concurrency::ValkeyConcurrencyMirror::from_config(
                    config,
                )
                .ok()
            }) {
                Some(mirror) => std::sync::Arc::new(
                    deepref_app::mirrored_concurrency::MirroredAiConcurrencyAdmission::new(
                        canonical,
                        std::sync::Arc::new(mirror),
                    )
                    .with_observer(std::sync::Arc::new(
                        deepref_telemetry::operational::OperationalMetrics::default(),
                    )),
                )
                    as std::sync::Arc<dyn deepref_ai::AiConcurrencyAdmission>,
                None => canonical,
            };
            let gateway = deepref_ai::build_metered_provider(
                provider.id(),
                &ai_config.base_url,
                api_key,
                std::sync::Arc::new(deepref_app::PostgresUsageLedger::new(&pool)),
                price_book(ai_config),
                admission,
                deepref_ai::AiConcurrencyLimits {
                    workspace: ai_config.max_concurrent_per_workspace,
                    route: ai_config.max_concurrent_per_route,
                },
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
