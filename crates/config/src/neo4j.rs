use std::{fs, time::Duration};

use crate::{ConfigError, Environment, database::seconds, optional, parse};

#[derive(Debug, Clone)]
pub struct Neo4jConfig {
    pub uri: String,
    pub user: String,
    pub password: String,
    pub connect_timeout: Duration,
    pub query_timeout: Duration,
    pub pool_max: usize,
}

impl Neo4jConfig {
    pub(crate) fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let uri = optional(get, "NEO4J_URI")
            .or_else(|| {
                environment
                    .is_local()
                    .then(|| "bolt://127.0.0.1:7687".into())
            })
            .ok_or(ConfigError::Missing("NEO4J_URI", environment))?;
        let user = optional(get, "NEO4J_USER")
            .or_else(|| environment.is_local().then(|| "neo4j".into()))
            .ok_or(ConfigError::Missing("NEO4J_USER", environment))?;
        let direct = optional(get, "NEO4J_PASSWORD");
        let file = optional(get, "NEO4J_PASSWORD_FILE");
        if direct.is_some() && file.is_some() {
            return Err(ConfigError::MutuallyExclusive(
                "NEO4J_PASSWORD",
                "NEO4J_PASSWORD_FILE",
            ));
        }
        let password = match (direct, file) {
            (Some(value), None) => value,
            (None, Some(path)) => fs::read_to_string(&path)
                .map_err(|source| ConfigError::SecretFile {
                    name: "NEO4J_PASSWORD_FILE",
                    path,
                    source,
                })?
                .trim()
                .to_owned(),
            (None, None) if environment.is_local() => "deepref-local".into(),
            (None, None) => return Err(ConfigError::Missing("NEO4J_PASSWORD", environment)),
            (Some(_), Some(_)) => unreachable!(),
        };
        let pool_max = parse(get, "NEO4J_POOL_MAX", 16_usize)?;
        if password.is_empty() || pool_max == 0 {
            return Err(ConfigError::Invalid {
                name: "NEO4J_PASSWORD",
                reason: "password and pool size must be non-empty".into(),
            });
        }
        Ok(Self {
            uri,
            user,
            password,
            connect_timeout: seconds(get, "NEO4J_CONNECT_TIMEOUT_SECS", 5)?,
            query_timeout: seconds(get, "NEO4J_QUERY_TIMEOUT_SECS", 15)?,
            pool_max,
        })
    }
}
