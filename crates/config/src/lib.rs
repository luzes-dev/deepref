mod database;
mod environment;
mod error;
mod telemetry;

use std::collections::HashMap;

pub use database::DatabaseConfig;
pub use environment::Environment;
pub use error::ConfigError;
pub use telemetry::TelemetryConfig;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub environment: Environment,
    pub database: DatabaseConfig,
    pub telemetry: TelemetryConfig,
}

impl RuntimeConfig {
    pub fn from_env(service_name: &str) -> Result<Self, ConfigError> {
        Self::from_source(service_name, &|name| std::env::var(name).ok())
    }

    pub fn from_map(
        service_name: &str,
        values: &HashMap<String, String>,
    ) -> Result<Self, ConfigError> {
        Self::from_source(service_name, &|name| values.get(name).cloned())
    }

    fn from_source(
        service_name: &str,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let environment = Environment::parse(get("APP_ENV").as_deref().unwrap_or("local"))?;
        Ok(Self {
            database: DatabaseConfig::from_source(environment, get)?,
            telemetry: TelemetryConfig::from_source(service_name, get)?,
            environment,
        })
    }
}

pub(crate) fn optional(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    get(name)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn parse<T>(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default: T,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    get(name).map_or(Ok(default), |value| {
        value.parse::<T>().map_err(|error| ConfigError::Invalid {
            name,
            reason: error.to_string(),
        })
    })
}
