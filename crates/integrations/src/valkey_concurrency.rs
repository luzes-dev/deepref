//! TTL mirrors of exact PostgreSQL slot grants; never a dispatch authority.

use deepref_application::concurrency_mirror::{
    ConcurrencyMirror, MirrorDecision, MirrorFuture, MirrorLeaseId, MirrorPolicy, MirrorScope,
    MirrorUnavailable,
};
use deepref_config::ValkeyConfig;

use crate::valkey_client::ValkeyClient;

const CHECK: &str = r"
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[1]) then return 0 end
return 1
";

const HOLD: &str = r"
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
local exists = redis.call('ZSCORE', KEYS[1], ARGV[2])
if not exists and redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[1]) then return 0 end
redis.call('ZADD', KEYS[1], now + tonumber(ARGV[3]), ARGV[2])
redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[3]))
return 1
";

const REFRESH: &str = r"
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local expiry = redis.call('ZSCORE', KEYS[1], ARGV[1])
if not expiry or tonumber(expiry) <= now then return 0 end
redis.call('ZADD', KEYS[1], now + tonumber(ARGV[2]), ARGV[1])
redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[2]))
return 1
";

#[derive(Clone)]
pub struct ValkeyConcurrencyMirror {
    client: ValkeyClient,
}

impl ValkeyConcurrencyMirror {
    pub fn from_config(config: &ValkeyConfig) -> Result<Self, MirrorUnavailable> {
        Ok(Self {
            client: ValkeyClient::from_config(config).map_err(|_| MirrorUnavailable)?,
        })
    }

    async fn run(
        &self,
        script: &str,
        scope: MirrorScope,
        args: &[String],
    ) -> Result<i64, MirrorUnavailable> {
        let mut connection = self
            .client
            .connection()
            .await
            .map_err(|_| MirrorUnavailable)?;
        let script = redis::Script::new(script);
        let mut invocation = script.prepare_invoke();
        invocation.key(mirror_key(&self.client.namespace, scope));
        for arg in args {
            invocation.arg(arg);
        }
        tokio::time::timeout(
            self.client.timeout,
            invocation.invoke_async(&mut connection),
        )
        .await
        .map_err(|_| MirrorUnavailable)?
        .map_err(|_| MirrorUnavailable)
    }
}

pub(crate) fn mirror_key(namespace: &str, scope: MirrorScope) -> String {
    match scope {
        MirrorScope::Workspace(id) => format!(
            "deepref:{namespace}:concurrency:v1:workspace:{}",
            id.as_uuid()
        ),
        MirrorScope::Route(digest) => {
            let digest = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("deepref:{namespace}:concurrency:v1:route:{digest}")
        }
    }
}

fn member(lease: MirrorLeaseId) -> String {
    format!("{}:{}:{}", lease.holder, lease.generation, lease.slot)
}

fn decision(value: i64) -> Result<MirrorDecision, MirrorUnavailable> {
    match value {
        1 => Ok(MirrorDecision::Available),
        0 => Ok(MirrorDecision::Saturated),
        _ => Err(MirrorUnavailable),
    }
}

impl ConcurrencyMirror for ValkeyConcurrencyMirror {
    fn check(&self, scope: MirrorScope, policy: MirrorPolicy) -> MirrorFuture<'_, MirrorDecision> {
        Box::pin(async move {
            decision(
                self.run(CHECK, scope, &[policy.limit().to_string()])
                    .await?,
            )
        })
    }

    fn hold(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, MirrorDecision> {
        Box::pin(async move {
            decision(
                self.run(
                    HOLD,
                    scope,
                    &[
                        policy.limit().to_string(),
                        member(lease),
                        policy.ttl().as_millis().to_string(),
                    ],
                )
                .await?,
            )
        })
    }

    fn refresh(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, ()> {
        Box::pin(async move {
            self.run(
                REFRESH,
                scope,
                &[member(lease), policy.ttl().as_millis().to_string()],
            )
            .await?;
            Ok(())
        })
    }

    fn release(&self, scope: MirrorScope, lease: MirrorLeaseId) -> MirrorFuture<'_, ()> {
        Box::pin(async move {
            self.run(
                "return redis.call('ZREM', KEYS[1], ARGV[1])",
                scope,
                &[member(lease)],
            )
            .await?;
            Ok(())
        })
    }
}
