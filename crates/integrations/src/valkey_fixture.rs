//! Test-only control of an owned loopback namespace. No SDK types escape the
//! integration adapter, and no wildcard/global deletion is exposed.

use crate::valkey::ValkeyRateAccelerator;
use deepref_application::rate_limits::{RateLimitSubject, RateLimitUnavailable};
use deepref_config::{ExposeSecret, ValkeyConfig};
use redis::aio::MultiplexedConnection;

pub struct ValkeyFixtureControl {
    connection: MultiplexedConnection,
    namespace: String,
    endpoint: String,
    config: ValkeyConfig,
}

impl ValkeyFixtureControl {
    pub async fn connect(
        address: &str,
        namespace: &str,
        timeout: std::time::Duration,
    ) -> Result<Self, RateLimitUnavailable> {
        let config = ValkeyConfig {
            url: address.to_owned().into(),
            namespace: namespace.to_owned(),
            timeout,
        };
        let client =
            redis::Client::open(config.url.expose_secret()).map_err(|_| RateLimitUnavailable)?;
        let endpoint = match client.get_connection_info().addr() {
            redis::ConnectionAddr::Tcp(host, port) if host == "127.0.0.1" => {
                format!("{host}:{port}")
            }
            _ => return Err(RateLimitUnavailable),
        };
        if !config.namespace.starts_with("contract-") {
            return Err(RateLimitUnavailable);
        }
        let connection = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.get_multiplexed_async_connection(),
        )
        .await
        .map_err(|_| RateLimitUnavailable)?
        .map_err(|_| RateLimitUnavailable)?;
        Ok(Self {
            connection,
            namespace: config.namespace.clone(),
            endpoint,
            config,
        })
    }

    pub fn accelerator(&self) -> Result<ValkeyRateAccelerator, RateLimitUnavailable> {
        ValkeyRateAccelerator::from_config(&self.config)
    }

    pub fn concurrency_mirror(
        &self,
    ) -> Result<
        crate::valkey_concurrency::ValkeyConcurrencyMirror,
        deepref_application::concurrency_mirror::MirrorUnavailable,
    > {
        crate::valkey_concurrency::ValkeyConcurrencyMirror::from_config(&self.config)
    }

    pub async fn clear_concurrency(
        &mut self,
        scope: deepref_application::concurrency_mirror::MirrorScope,
    ) -> Result<(), RateLimitUnavailable> {
        redis::cmd("DEL")
            .arg(crate::valkey_concurrency::mirror_key(
                &self.namespace,
                scope,
            ))
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }

    pub async fn concurrency_count(
        &mut self,
        scope: deepref_application::concurrency_mirror::MirrorScope,
    ) -> Result<i64, RateLimitUnavailable> {
        redis::cmd("ZCARD")
            .arg(crate::valkey_concurrency::mirror_key(
                &self.namespace,
                scope,
            ))
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }

    pub fn lazy_accelerator(
        address: String,
        namespace: String,
        timeout: std::time::Duration,
    ) -> Result<ValkeyRateAccelerator, RateLimitUnavailable> {
        ValkeyRateAccelerator::from_config(&ValkeyConfig {
            url: address.into(),
            namespace,
            timeout,
        })
    }

    pub fn lazy_concurrency_mirror(
        address: String,
        namespace: String,
        timeout: std::time::Duration,
    ) -> Result<
        crate::valkey_concurrency::ValkeyConcurrencyMirror,
        deepref_application::concurrency_mirror::MirrorUnavailable,
    > {
        crate::valkey_concurrency::ValkeyConcurrencyMirror::from_config(&ValkeyConfig {
            url: address.into(),
            namespace,
            timeout,
        })
    }

    fn key(&self, subject: RateLimitSubject) -> String {
        format!(
            "deepref:{}:rate:v1:{}:{}",
            self.namespace,
            subject.kind(),
            subject.id()
        )
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn ttl(&mut self, subject: RateLimitSubject) -> Result<i64, RateLimitUnavailable> {
        redis::cmd("PTTL")
            .arg(self.key(subject))
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }

    pub async fn expire_now(
        &mut self,
        subject: RateLimitSubject,
    ) -> Result<(), RateLimitUnavailable> {
        redis::cmd("PEXPIRE")
            .arg(self.key(subject))
            .arg(1)
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }

    pub async fn clear(&mut self, subject: RateLimitSubject) -> Result<(), RateLimitUnavailable> {
        redis::cmd("DEL")
            .arg(self.key(subject))
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }

    pub async fn pause_writes(&mut self) -> Result<(), RateLimitUnavailable> {
        redis::cmd("CLIENT")
            .arg("PAUSE")
            .arg(300)
            .arg("WRITE")
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }

    pub async fn used(&mut self, subject: RateLimitSubject) -> Result<i64, RateLimitUnavailable> {
        redis::cmd("HGET")
            .arg(self.key(subject))
            .arg("used")
            .query_async(&mut self.connection)
            .await
            .map_err(|_| RateLimitUnavailable)
    }
}
