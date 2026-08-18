use deepref_projector::config::{ProjectorCommand, ProjectorConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ProjectorConfig::from_env(std::env::args().skip(1))?;
    let telemetry = deepref_telemetry::init(config.runtime.telemetry.clone())?;
    let pool = deepref_projector::connect_database(&config).await?;
    let graph = deepref_projector::connect_graph(&config).await?;
    let result = match config.command.clone() {
        ProjectorCommand::Run => deepref_projector::run(&config, pool, graph).await,
        ProjectorCommand::Migrate => graph.apply_migrations().await.map_err(Into::into),
        ProjectorCommand::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&deepref_projector::status::snapshot(&pool).await?)?
            );
            Ok(())
        }
        ProjectorCommand::Rebuild { run_id } => {
            deepref_projector::rebuild::run(
                &pool,
                &graph,
                run_id,
                config.batch_size,
                config.advisory_lock_key,
            )
            .await
        }
    };
    telemetry.shutdown().await;
    result
}
