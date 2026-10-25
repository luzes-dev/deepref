//! Shared bounded, lazy Valkey connection, private to adapter implementations.
use deepref_application::rate_limits::RateLimitUnavailable;
use deepref_config::{ExposeSecret, ValkeyConfig};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use std::{sync::Arc, time::Duration};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct ValkeyClient {
    client: redis::Client,
    connection: Arc<OnceCell<ConnectionManager>>,
    pub(crate) namespace: Arc<str>,
    pub(crate) timeout: Duration,
}

impl ValkeyClient {
    /// No network side effect. A startup outage must not permanently disable
    /// acceleration: a later request can initialize the bounded client.
    pub fn from_config(config: &ValkeyConfig) -> Result<Self, RateLimitUnavailable> {
        let client =
            redis::Client::open(config.url.expose_secret()).map_err(|_| RateLimitUnavailable)?;
        Ok(Self {
            client,
            connection: Arc::new(OnceCell::new()),
            namespace: Arc::from(config.namespace.as_str()),
            timeout: config.timeout,
        })
    }

    pub(crate) async fn connection(&self) -> Result<ConnectionManager, RateLimitUnavailable> {
        let options = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(self.timeout))
            .set_response_timeout(Some(self.timeout))
            .set_number_of_retries(0)
            .set_concurrency_limit(64);
        tokio::time::timeout(
            self.timeout,
            self.connection.get_or_try_init(|| async {
                ConnectionManager::new_with_config(self.client.clone(), options)
                    .await
                    .map_err(|_| RateLimitUnavailable)
            }),
        )
        .await
        .map_err(|_| RateLimitUnavailable)?
        .cloned()
    }
}
