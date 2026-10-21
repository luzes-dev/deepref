use std::future::Future;

use tokio::sync::watch;

pub mod command;

pub use command::{AiUsageCommand, AiUsageResolutionCommand, Command, LegacyActorCommand};

pub async fn run(command: Command) -> anyhow::Result<()> {
    let source = deepref_config::ConfigSource::from_env();
    let get = |name: &str| source.get(name);
    match command {
        Command::Serve => {
            let config = deepref_http_api::config::ApiConfig::from_source(&get)?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_http_api::serve(config),
            )
            .await
        }
        Command::Worker { pools } => {
            let config = deepref_worker::config::WorkerConfig::from_source(&get)?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_worker::run(config, pools),
            )
            .await
        }
        Command::All { pools } => {
            let api = deepref_http_api::config::ApiConfig::from_source(&get)?;
            let worker = deepref_worker::config::WorkerConfig::from_source(&get)?;
            with_telemetry(
                api.runtime.telemetry.clone(),
                run_all(api, worker, pools, wait_for_signal()),
            )
            .await
        }
        Command::Migrate => {
            let config = deepref_http_api::config::ApiConfig::from_source(&get)?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_http_api::migrate(&config),
            )
            .await
        }

        Command::BackfillProjectWorkspaces => {
            let config = deepref_http_api::config::ApiConfig::from_source(&get)?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_http_api::backfill_project_workspaces(&config),
            )
            .await
        }

        Command::LegacyActor(action) => run_legacy_actor(action).await,

        Command::AiUsage(action) => run_ai_usage(action).await,

        Command::PrintOpenApi => {
            println!(
                "{}",
                serde_json::to_string_pretty(&deepref_http_api::routes::openapi_document_json()?)?
            );
            Ok(())
        }
    }
}

async fn run_ai_usage(action: AiUsageCommand) -> anyhow::Result<()> {
    let source = deepref_config::ConfigSource::from_env();
    let config = deepref_http_api::config::ApiConfig::from_source(&|name| source.get(name))?;
    let telemetry = config.runtime.telemetry.clone();
    with_telemetry(telemetry, async move {
        let output = match action {
            AiUsageCommand::Backfill { batch_size } => {
                deepref_http_api::ai_usage_backfill(&config, batch_size).await?
            }
            AiUsageCommand::EnableHardCaps {
                operator_user_id,
                quiet_window_seconds,
            } => {
                deepref_http_api::ai_usage_enable_hard_caps(
                    &config,
                    operator_user_id,
                    quiet_window_seconds,
                )
                .await?
            }
            AiUsageCommand::DisableHardCaps { operator_user_id } => {
                deepref_http_api::ai_usage_disable_hard_caps(&config, operator_user_id).await?
            }
            AiUsageCommand::Resolve {
                reservation_id,
                operator_user_id,
                resolution,
            } => {
                let (actual_cost_micros, reason) = match resolution {
                    AiUsageResolutionCommand::Settle { actual_cost_micros } => {
                        (Some(actual_cost_micros), None)
                    }
                    AiUsageResolutionCommand::Void { reason } => (None, Some(reason)),
                };
                deepref_http_api::ai_usage_resolve(
                    &config,
                    reservation_id,
                    operator_user_id,
                    actual_cost_micros,
                    reason,
                )
                .await?
            }
            AiUsageCommand::Reconcile { workspace_id } => {
                deepref_http_api::ai_usage_reconcile(&config, workspace_id).await?
            }
            AiUsageCommand::ExportReplay { since, limit } => {
                deepref_http_api::ai_usage_export_replay(&config, since, limit).await?
            }
            AiUsageCommand::WaiveOverage {
                workspace_id,
                operator_user_id,
            } => {
                deepref_http_api::ai_usage_waive_overage(&config, workspace_id, operator_user_id)
                    .await?
            }
            AiUsageCommand::SetCap {
                scope_kind,
                scope_id,
                limit_micros,
                operator_user_id,
            } => {
                deepref_http_api::ai_usage_set_cap(
                    &config,
                    &scope_kind,
                    scope_id,
                    limit_micros,
                    operator_user_id,
                )
                .await?
            }
        };
        println!("{output}");
        Ok(())
    })
    .await
}

async fn run_legacy_actor(action: LegacyActorCommand) -> anyhow::Result<()> {
    let source = deepref_config::ConfigSource::from_env();
    let config = deepref_http_api::config::ApiConfig::from_source(&|name| source.get(name))?;
    let telemetry = config.runtime.telemetry.clone();
    with_telemetry(telemetry, async move {
        let result = match action {
            LegacyActorCommand::Map {
                kind,
                legacy_id,
                user_id,
                operator_user_id,
                source,
            } => {
                deepref_http_api::legacy_actor_map(
                    &config,
                    &kind,
                    &legacy_id,
                    user_id,
                    operator_user_id,
                    &source,
                )
                .await?
            }
            LegacyActorCommand::Deny {
                kind,
                legacy_id,
                user_id,
                operator_user_id,
                source,
            } => {
                deepref_http_api::legacy_actor_deny(
                    &config,
                    &kind,
                    &legacy_id,
                    user_id,
                    operator_user_id,
                    &source,
                )
                .await?
            }
            LegacyActorCommand::List => deepref_http_api::legacy_actor_list(&config).await?,
        };
        println!("{result}");
        Ok(())
    })
    .await
}

async fn with_telemetry<F>(
    config: deepref_config::TelemetryConfig,
    operation: F,
) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    let telemetry = deepref_telemetry::init(config)?;
    let result = operation.await;
    telemetry.shutdown().await;
    result
}

async fn run_all(
    api: deepref_http_api::config::ApiConfig,
    worker: deepref_worker::config::WorkerConfig,
    pools: Vec<deepref_domain::WorkloadClass>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let api_shutdown = wait_for_notification(shutdown_rx.clone());
    let worker_shutdown = wait_for_notification(shutdown_rx);
    let mut api_task = tokio::spawn(deepref_http_api::serve_with_shutdown(api, api_shutdown));
    let mut worker_task = tokio::spawn(deepref_worker::run_with_shutdown(
        worker,
        pools,
        worker_shutdown,
    ));
    let mut shutdown = Box::pin(shutdown);

    let first = tokio::select! {
        result = &mut api_task => FirstFinished::Api(join_result(result)),
        result = &mut worker_task => FirstFinished::Worker(join_result(result)),
        _ = &mut shutdown => {
            shutdown_tx.send_replace(true);
            let api_result = join_result(api_task.await);
            let worker_result = join_result(worker_task.await);
            return combine_results(api_result, worker_result);
        }
    };

    shutdown_tx.send_replace(true);
    match first {
        FirstFinished::Api(api_result) => {
            combine_results(api_result, join_result(worker_task.await))
        }
        FirstFinished::Worker(worker_result) => {
            combine_results(join_result(api_task.await), worker_result)
        }
    }
}

enum FirstFinished {
    Api(anyhow::Result<anyhow::Result<()>>),
    Worker(anyhow::Result<anyhow::Result<()>>),
}

fn join_result(
    result: Result<anyhow::Result<()>, tokio::task::JoinError>,
) -> anyhow::Result<anyhow::Result<()>> {
    result.map_err(Into::into)
}

fn combine_results(
    first: anyhow::Result<anyhow::Result<()>>,
    second: anyhow::Result<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    first??;
    second??;
    Ok(())
}

async fn wait_for_notification(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

async fn wait_for_signal() {
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
