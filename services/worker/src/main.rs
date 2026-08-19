use deepref_worker::config::WorkerConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = WorkerConfig::from_env()?;
    let telemetry = deepref_telemetry::init(config.runtime.telemetry.clone())?;
    let result = deepref_worker::run(config).await;
    telemetry.shutdown().await;
    result
}
