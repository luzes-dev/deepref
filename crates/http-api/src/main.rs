use deepref_http_api::config::{ApiCommand, ApiConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = ApiCommand::parse(std::env::args().skip(1))?;
    if command == ApiCommand::PrintOpenApi {
        println!(
            "{}",
            serde_json::to_string_pretty(&deepref_http_api::routes::openapi_document())?
        );
        return Ok(());
    }
    let config = ApiConfig::from_env()?;
    let telemetry = deepref_telemetry::init(config.runtime.telemetry.clone())?;
    let result = match command {
        ApiCommand::Serve => deepref_http_api::serve(config).await,
        ApiCommand::Migrate => deepref_http_api::migrate(&config).await,
        ApiCommand::PrintOpenApi => unreachable!(),
    };
    telemetry.shutdown().await;
    result
}
