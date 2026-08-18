use async_nats::{
    ConnectOptions,
    jetstream::{self, consumer::PullConsumer},
};
use deepref_config::NatsConfig;
use deepref_events::STREAM_DOMAIN;

pub async fn connect(config: &NatsConfig) -> anyhow::Result<jetstream::Context> {
    let mut options = ConnectOptions::new().connection_timeout(config.connect_timeout);
    if let Some(path) = &config.credentials_file {
        options = options.credentials_file(path).await?;
    }
    if let Some(path) = &config.ca_file {
        options = options.add_root_certificates(path.into()).require_tls(true);
    }
    let client =
        tokio::time::timeout(config.connect_timeout, options.connect(&config.url)).await??;
    Ok(jetstream::new(client))
}

pub async fn bind(
    context: &jetstream::Context,
    durable_name: &str,
) -> anyhow::Result<PullConsumer> {
    let stream = context.get_stream(STREAM_DOMAIN).await?;
    stream
        .get_consumer(durable_name)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}
