//! Bounded Valkey acceleration. Use behind the canonical PostgreSQL limiter;
//! this adapter alone is not a safe fallback/cutover protocol.

use deepref_application::rate_limits::{
    RateLimitDecision, RateLimitFuture, RateLimitPolicy, RateLimitSubject, RateLimitUnavailable,
    RateLimiter,
};
use deepref_config::ValkeyConfig;

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
    client: crate::valkey_client::ValkeyClient,
}

impl ValkeyRateAccelerator {
    pub fn from_config(config: &ValkeyConfig) -> Result<Self, RateLimitUnavailable> {
        Ok(Self {
            client: crate::valkey_client::ValkeyClient::from_config(config)?,
        })
    }

    pub async fn connect(config: &ValkeyConfig) -> Result<Self, RateLimitUnavailable> {
        let adapter = Self::from_config(config)?;
        adapter.client.connection().await?;
        Ok(adapter)
    }
}

impl RateLimiter for ValkeyRateAccelerator {
    fn check(&self, subject: RateLimitSubject, policy: RateLimitPolicy) -> RateLimitFuture<'_> {
        Box::pin(async move {
            let key = format!(
                "deepref:{}:rate:v1:{}:{}",
                self.client.namespace,
                subject.kind(),
                subject.id()
            );
            let mut connection = self.client.connection().await?;
            let value: i64 = tokio::time::timeout(
                self.client.timeout,
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
