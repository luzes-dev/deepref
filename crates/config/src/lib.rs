mod ai;
mod api;
mod database;
mod deletion_digest;
mod document;
mod environment;
mod error;
mod integrations;
mod listen;
mod nats;
mod origin;
mod pdf;
mod polar;
mod smtp;
mod source;
mod telemetry;
mod usage_export;
mod worker;
mod workflow;
mod workos;

use std::collections::HashMap;

pub use ai::{
    AiProviderConfig, AiProviderKind, DEFAULT_AI_MODEL, DEFAULT_OPENCODE_BASE_URL,
    DEFAULT_ZAI_BASE_URL, ModelPriceOverride,
};
pub use api::ApiSettings;
pub use database::DatabaseConfig;
pub use deletion_digest::DeletionDigestConfig;
pub use document::{
    DEFAULT_MAX_DOCUMENT_BYTES, DocumentConfig, DocumentStorageBackend, MAX_DOCUMENT_BYTES,
};
pub use environment::{APP_ENV, Environment, load_development_dotenv_files};
pub use error::ConfigError;
pub use integrations::{IntegrationsConfig, OpenAlexCorpus};
pub use listen::{ALLOW_NETWORK_BIND, API_BIND_ADDR, DEFAULT_API_BIND_ADDR, ListenConfig};
pub use nats::{NatsConfig, WorkTransportConfig};
pub use origin::{DEFAULT_DEVELOPMENT_PUBLIC_ORIGIN, PUBLIC_ORIGIN, PublicOrigin};
pub use pdf::PdfConfig;
pub use polar::{PolarConfig, PolarServer};
pub use secrecy::{ExposeSecret, SecretString};
pub use smtp::{SmtpConfig, SmtpTls};
pub use source::ConfigSource;
pub use telemetry::TelemetryConfig;
pub use usage_export::{USAGE_EXPORT_ENABLED, USAGE_EXPORT_REPLAY_BATCH_SIZE, UsageExportConfig};
pub use worker::WorkerSettings;
pub use workflow::WorkflowConfig;
pub use workos::WorkOsConfig;

/// Turns authentication on. Off until the WorkOS handoff ships; in production
/// it requires a complete [`WorkOsConfig`].
pub const AUTH_ENABLED: &str = "DEEPREF_AUTH_ENABLED";

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub environment: Environment,
    pub public_origin: PublicOrigin,
    pub auth_enabled: bool,
    pub workos: Option<WorkOsConfig>,
    pub polar: Option<PolarConfig>,
    pub database: DatabaseConfig,
    pub telemetry: TelemetryConfig,
    pub workflow: WorkflowConfig,
    pub smtp: SmtpConfig,
    pub ai: AiProviderConfig,
    pub pdf: PdfConfig,
    pub integrations: IntegrationsConfig,
    pub usage_export: UsageExportConfig,
    pub work_transport: WorkTransportConfig,
}

impl RuntimeConfig {
    /// The same gate used by the API session layer for its fixed local user.
    /// It is enabled only when authentication is explicitly off in development.
    pub const fn development_principal_enabled(&self) -> bool {
        self.environment.is_development() && !self.auth_enabled
    }

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
        let auth_enabled = flag(get, AUTH_ENABLED)?;
        Ok(Self {
            public_origin: PublicOrigin::from_source(environment, get)?,
            workos: WorkOsConfig::from_source(environment, auth_enabled, get)?,
            polar: PolarConfig::from_source(environment, get)?,
            auth_enabled,
            database: DatabaseConfig::from_source(environment, get)?,
            telemetry: TelemetryConfig::from_source(service_name, get)?,
            workflow: WorkflowConfig::from_source(get),
            smtp: SmtpConfig::from_source(get),
            ai: AiProviderConfig::from_source(get)?,
            pdf: PdfConfig::from_source(get),
            integrations: IntegrationsConfig::from_source(get)?,
            usage_export: UsageExportConfig::from_source(environment, get)?,
            work_transport: WorkTransportConfig::from_source(environment, get)?,
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

pub(crate) fn flag(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<bool, ConfigError> {
    match optional(get, name)
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("0" | "false" | "no") => Ok(false),
        Some("1" | "true" | "yes") => Ok(true),
        Some(value) => Err(ConfigError::Invalid {
            name,
            reason: format!("{value:?} is not one of 1, true, yes, 0, false, no"),
        }),
    }
}

/// Reads an all-or-nothing group of variables. `None` means none are set; a
/// subset is an error naming what is missing. Values come back trimmed, in the
/// order of `names`.
pub(crate) fn section<const N: usize>(
    section: &'static str,
    get: &dyn Fn(&str) -> Option<String>,
    names: [&'static str; N],
) -> Result<Option<[String; N]>, ConfigError> {
    let values = names.map(|name| optional(get, name));
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let missing: Vec<_> = names
        .iter()
        .zip(&values)
        .filter_map(|(name, value)| value.is_none().then_some(*name))
        .collect();
    if !missing.is_empty() {
        return Err(ConfigError::PartialSection { section, missing });
    }
    Ok(Some(values.map(Option::unwrap_or_default)))
}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    #[test]
    fn development_principal_uses_the_session_gate_conditions() {
        let development = RuntimeConfig::from_map(
            "development-principal-test",
            &HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]),
        )
        .expect("development config");
        assert!(development.development_principal_enabled());

        let mut authenticated = development.clone();
        authenticated.auth_enabled = true;
        assert!(!authenticated.development_principal_enabled());

        let mut production = development;
        production.environment = Environment::Production;
        assert!(!production.development_principal_enabled());
    }
}
