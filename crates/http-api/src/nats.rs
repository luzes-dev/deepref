use std::path::PathBuf;

use async_nats::{ConnectOptions, jetstream};
use deepref_config::NatsConfig;

/// Connects and returns a publish context. Stream and consumer ownership lives
/// exclusively in local bootstrap/Helm.
pub async fn connect_jetstream(config: &NatsConfig) -> anyhow::Result<jetstream::Context> {
    let mut options = ConnectOptions::new().connection_timeout(config.connect_timeout);
    if let Some(path) = &config.credentials_file {
        options = options.credentials_file(path).await?;
    }
    if let Some(path) = &config.ca_file {
        options = options
            .add_root_certificates(PathBuf::from(path))
            .require_tls(true);
    }
    let client =
        tokio::time::timeout(config.connect_timeout, options.connect(&config.url)).await??;
    Ok(jetstream::new(client))
}
