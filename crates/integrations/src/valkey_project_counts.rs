//! Cache only bounded, tenant-scoped article-count display snapshots.

use chrono::Utc;
use deepref_application::project_count_cache::{
    ArticleCountSnapshot, COUNT_CACHE_MAX_BATCH, COUNT_CACHE_MAX_VALUE_BYTES, COUNT_CACHE_TTL,
    CountCacheFuture, CountCacheUnavailable, ProjectCountCache, ProjectCountKey,
};
use deepref_config::ValkeyConfig;

use crate::valkey_client::ValkeyClient;

// Bound values on the server before GET: checking size after a normal MGET
// would already have allocated an arbitrarily large corrupt cache response.
const READ: &str = r"
local result = {}
for i, key in ipairs(KEYS) do
  if redis.call('STRLEN', key) <= tonumber(ARGV[1]) then
    result[i] = redis.call('GET', key) or false
  else result[i] = false end
end
return result
";

const WRITE: &str = r"
for i, key in ipairs(KEYS) do
  redis.call('SET', key, ARGV[i + 1], 'PX', ARGV[1])
end
return #KEYS
";

#[derive(Clone)]
pub struct ValkeyProjectCountCache {
    client: ValkeyClient,
}

impl ValkeyProjectCountCache {
    pub fn from_config(config: &ValkeyConfig) -> Result<Self, CountCacheUnavailable> {
        Ok(Self {
            client: ValkeyClient::from_config(config).map_err(|_| CountCacheUnavailable)?,
        })
    }
}

pub(crate) fn count_key(namespace: &str, key: ProjectCountKey) -> String {
    format!(
        "deepref:{namespace}:project-count:v1:{}:{}:{}",
        key.workspace.as_uuid(),
        key.project.as_uuid(),
        key.revision
    )
}

impl ProjectCountCache for ValkeyProjectCountCache {
    fn get<'a>(
        &'a self,
        keys: &'a [ProjectCountKey],
    ) -> CountCacheFuture<'a, Vec<Option<ArticleCountSnapshot>>> {
        Box::pin(async move {
            if keys.is_empty() {
                return Ok(Vec::new());
            }
            if keys.len() > COUNT_CACHE_MAX_BATCH || keys.iter().any(|key| key.revision < 0) {
                return Err(CountCacheUnavailable);
            }
            let mut connection = self
                .client
                .connection()
                .await
                .map_err(|_| CountCacheUnavailable)?;
            let script = redis::Script::new(READ);
            let mut invocation = script.prepare_invoke();
            for key in keys {
                invocation.key(count_key(&self.client.namespace, *key));
            }
            invocation.arg(COUNT_CACHE_MAX_VALUE_BYTES);
            let values: Vec<Option<Vec<u8>>> = tokio::time::timeout(
                self.client.timeout,
                invocation.invoke_async(&mut connection),
            )
            .await
            .map_err(|_| CountCacheUnavailable)?
            .map_err(|_| CountCacheUnavailable)?;
            if values.len() != keys.len() {
                return Err(CountCacheUnavailable);
            }
            let now = Utc::now();
            Ok(values
                .into_iter()
                .zip(keys)
                .map(|(value, key)| {
                    let value: ArticleCountSnapshot = serde_json::from_slice(&value?).ok()?;
                    value.matches_at(*key, now).then_some(value)
                })
                .collect())
        })
    }

    fn put<'a>(&'a self, snapshots: &'a [ArticleCountSnapshot]) -> CountCacheFuture<'a, ()> {
        Box::pin(async move {
            if snapshots.is_empty() {
                return Ok(());
            }
            if snapshots.len() > COUNT_CACHE_MAX_BATCH {
                return Err(CountCacheUnavailable);
            }
            let now = Utc::now();
            let encoded: Vec<_> = snapshots
                .iter()
                .map(|snapshot| {
                    if !snapshot.matches_at(snapshot.key, now) {
                        return Err(CountCacheUnavailable);
                    }
                    let bytes = serde_json::to_vec(snapshot).map_err(|_| CountCacheUnavailable)?;
                    if bytes.len() > COUNT_CACHE_MAX_VALUE_BYTES {
                        return Err(CountCacheUnavailable);
                    }
                    Ok(bytes)
                })
                .collect::<Result<_, _>>()?;
            let mut connection = self
                .client
                .connection()
                .await
                .map_err(|_| CountCacheUnavailable)?;
            let script = redis::Script::new(WRITE);
            let mut invocation = script.prepare_invoke();
            for snapshot in snapshots {
                invocation.key(count_key(&self.client.namespace, snapshot.key));
            }
            invocation.arg(COUNT_CACHE_TTL.as_millis() as u64);
            for bytes in encoded {
                invocation.arg(bytes);
            }
            let _: i64 = tokio::time::timeout(
                self.client.timeout,
                invocation.invoke_async(&mut connection),
            )
            .await
            .map_err(|_| CountCacheUnavailable)?
            .map_err(|_| CountCacheUnavailable)?;
            Ok(())
        })
    }
}
