//! Bounded Valkey acceleration. Use behind the canonical PostgreSQL limiter;
//! this adapter alone is not a safe fallback/cutover protocol.

use std::{sync::Arc, time::Duration};

use deepref_application::rate_limits::{
    RateLimitDecision, RateLimitFuture, RateLimitPolicy, RateLimitSubject, RateLimitUnavailable,
    RateLimiter,
};
use deepref_config::{ExposeSecret, ValkeyConfig};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::sync::OnceCell;

// One hash per verified subject; expiry is anchored at the first attempt.
// Lower ceilings persist even on refused attempts. No mutation is retried by
// application code after an ambiguous response.
const CONSUME: &str = r"
local ceiling = tonumber(redis.call('HGET', KEYS[1], 'ceiling'))
local used = tonumber(redis.call('HGET', KEYS[1], 'used'))
local requested = tonumber(ARGV[1])
if not ceiling or not used then
  redis.call('HSET', KEYS[1], 'ceiling', requested, 'used', 1)
  redis.call('PEXPIRE', KEYS[1], 60000)
  return 1
end
ceiling = math.min(ceiling, requested)
redis.call('HSET', KEYS[1], 'ceiling', ceiling)
if used >= ceiling then return 0 end
redis.call('HINCRBY', KEYS[1], 'used', 1)
return 1
";

#[derive(Clone)]
pub struct ValkeyRateAccelerator {
    client: redis::Client,
    connection: Arc<OnceCell<ConnectionManager>>,
    namespace: Arc<str>,
    timeout: Duration,
}

impl ValkeyRateAccelerator {
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

    pub async fn connect(config: &ValkeyConfig) -> Result<Self, RateLimitUnavailable> {
        let adapter = Self::from_config(config)?;
        adapter.connection().await?;
        Ok(adapter)
    }

    async fn connection(&self) -> Result<ConnectionManager, RateLimitUnavailable> {
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

impl RateLimiter for ValkeyRateAccelerator {
    fn check(&self, subject: RateLimitSubject, policy: RateLimitPolicy) -> RateLimitFuture<'_> {
        Box::pin(async move {
            let key = format!(
                "deepref:{}:rate:v1:{}:{}",
                self.namespace,
                subject.kind(),
                subject.id()
            );
            let mut connection = self.connection().await?;
            let value: i64 = tokio::time::timeout(
                self.timeout,
                redis::Script::new(CONSUME)
                    .key(key)
                    .arg(policy.limit())
                    .invoke_async(&mut connection),
            )
            .await
            .map_err(|_| RateLimitUnavailable)?
            .map_err(|_| RateLimitUnavailable)?;
            match value {
                1 => Ok(RateLimitDecision::Allowed),
                0 => Ok(RateLimitDecision::Limited),
                _ => Err(RateLimitUnavailable),
            }
        })
    }
}
