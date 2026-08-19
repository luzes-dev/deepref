use std::future::Future;

use tokio::sync::watch;

pub mod command;

pub use command::Command;

pub async fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Serve => {
            let config = deepref_http_api::config::ApiConfig::from_env()?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_http_api::serve(config),
            )
            .await
        }
        Command::Worker => {
            let config = deepref_worker::config::WorkerConfig::from_env()?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_worker::run(config),
            )
            .await
        }
        Command::All => {
            let api = deepref_http_api::config::ApiConfig::from_env()?;
            let worker = deepref_worker::config::WorkerConfig::from_env()?;
            with_telemetry(
                api.runtime.telemetry.clone(),
                run_all(api, worker, wait_for_signal()),
            )
            .await
        }
        Command::Migrate => {
            let config = deepref_http_api::config::ApiConfig::from_env()?;
            with_telemetry(
                config.runtime.telemetry.clone(),
                deepref_http_api::migrate(&config),
            )
            .await
        }
        Command::ImportLegacy => {
            let config = deepref_http_api::config::ApiConfig::from_env()?;
            with_telemetry(config.runtime.telemetry.clone(), async move {
                let counts = deepref_http_api::import_legacy(&config).await?;
                println!("legacy import completed: {counts:?}");
                Ok(())
            })
            .await
        }
        Command::PrintOpenApi => {
            println!(
                "{}",
                serde_json::to_string_pretty(&deepref_http_api::routes::openapi_document())?
            );
            Ok(())
        }
    }
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
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let api_shutdown = wait_for_notification(shutdown_rx.clone());
    let worker_shutdown = wait_for_notification(shutdown_rx);
    let mut api_task = tokio::spawn(deepref_http_api::serve_with_shutdown(api, api_shutdown));
    let mut worker_task = tokio::spawn(deepref_worker::run_with_shutdown(worker, worker_shutdown));
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
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler must install");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
