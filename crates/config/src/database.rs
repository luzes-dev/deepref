use std::{fs, time::Duration};

use crate::{ConfigError, Environment, optional, parse};

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub pool_min: u32,
    pub pool_max: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
}

impl DatabaseConfig {
    pub(crate) fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let direct = optional(get, "DATABASE_URL");
        let file = optional(get, "DATABASE_URL_FILE");
        if direct.is_some() && file.is_some() {
            return Err(ConfigError::MutuallyExclusive(
                "DATABASE_URL",
                "DATABASE_URL_FILE",
            ));
        }
        let url = match (direct, file) {
            (Some(url), None) => url,
            (None, Some(path)) => fs::read_to_string(&path)
                .map_err(|source| ConfigError::SecretFile {
                    name: "DATABASE_URL_FILE",
                    path,
                    source,
                })?
                .trim()
                .to_owned(),
            (None, None) if environment.is_local() => {
                "postgres://postgres:postgres@127.0.0.1:5432/deepref".to_owned()
            }
            (None, None) => return Err(ConfigError::Missing("DATABASE_URL", environment)),
            (Some(_), Some(_)) => unreachable!(),
        };
        if url.is_empty() {
            return Err(ConfigError::Invalid {
                name: "DATABASE_URL",
                reason: "must not be blank".into(),
            });
        }
        let pool_min = parse(get, "DATABASE_POOL_MIN", 1_u32)?;
        let pool_max = parse(get, "DATABASE_POOL_MAX", 10_u32)?;
        if pool_max == 0 || pool_min > pool_max {
            return Err(ConfigError::Invalid {
                name: "DATABASE_POOL_MAX",
                reason: "must be non-zero and at least DATABASE_POOL_MIN".into(),
            });
        }
        Ok(Self {
            url,
            pool_min,
            pool_max,
            acquire_timeout: seconds(get, "DATABASE_ACQUIRE_TIMEOUT_SECS", 10)?,
            idle_timeout: seconds(get, "DATABASE_IDLE_TIMEOUT_SECS", 600)?,
            max_lifetime: seconds(get, "DATABASE_MAX_LIFETIME_SECS", 1_800)?,
        })
    }
}

pub(crate) fn seconds(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default: u64,
) -> Result<Duration, ConfigError> {
    let value = parse(get, name, default)?;
    if value == 0 {
        return Err(ConfigError::Invalid {
            name,
            reason: "must be greater than zero".into(),
        });
    }
    Ok(Duration::from_secs(value))
}
