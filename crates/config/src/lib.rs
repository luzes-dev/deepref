mod ai;
mod api;
mod database;
mod document;
mod environment;
mod error;
mod integrations;
mod listen;
mod pdf;
mod smtp;
mod source;
mod telemetry;
mod worker;
mod workflow;

use std::collections::HashMap;

pub use ai::{
    AiProviderConfig, AiProviderKind, DEFAULT_AI_MODEL, DEFAULT_OPENCODE_BASE_URL,
    DEFAULT_ZAI_BASE_URL, ModelPriceOverride,
};
pub use api::ApiSettings;
pub use database::DatabaseConfig;
pub use document::{
    DEFAULT_MAX_DOCUMENT_BYTES, DocumentConfig, DocumentStorageBackend, MAX_DOCUMENT_BYTES,
};
pub use environment::{APP_ENV, Environment, load_local_dotenv_files};
pub use error::ConfigError;
pub use integrations::{IntegrationsConfig, OpenAlexCorpus};
pub use listen::{ALLOW_NETWORK_BIND, API_BIND_ADDR, DEFAULT_API_BIND_ADDR, ListenConfig};
pub use pdf::PdfConfig;
pub use smtp::{SmtpConfig, SmtpTls};
pub use source::ConfigSource;
pub use telemetry::TelemetryConfig;
pub use worker::WorkerSettings;
pub use workflow::WorkflowConfig;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub environment: Environment,
    pub database: DatabaseConfig,
    pub telemetry: TelemetryConfig,
    pub workflow: WorkflowConfig,
    pub smtp: SmtpConfig,
    pub ai: AiProviderConfig,
    pub pdf: PdfConfig,
    pub integrations: IntegrationsConfig,
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

    pub fn from_source(
        service_name: &str,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let environment = Environment::from_app_env(get(APP_ENV).as_deref())?;
        Ok(Self {
            database: DatabaseConfig::from_source(environment, get)?,
            telemetry: TelemetryConfig::from_source(service_name, get)?,
            workflow: WorkflowConfig::from_source(get),
            smtp: SmtpConfig::from_source(get),
            ai: AiProviderConfig::from_source(get)?,
            pdf: PdfConfig::from_source(get),
            integrations: IntegrationsConfig::from_source(get)?,
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
