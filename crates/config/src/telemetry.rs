use std::time::Duration;

use crate::{ConfigError, database::seconds, optional};

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub service_name: String,
    pub log_filter: String,
    pub otlp_endpoint: Option<String>,
    pub shutdown_deadline: Duration,
}

impl TelemetryConfig {
    pub(crate) fn from_source(
        service_name: &str,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        if service_name.trim().is_empty() {
            return Err(ConfigError::Invalid {
                name: "OTEL_SERVICE_NAME",
                reason: "service name must not be blank".into(),
            });
        }
        Ok(Self {
            service_name: optional(get, "OTEL_SERVICE_NAME")
                .unwrap_or_else(|| service_name.to_owned()),
            log_filter: optional(get, "RUST_LOG").unwrap_or_else(|| "info".into()),
            otlp_endpoint: optional(get, "OTEL_EXPORTER_OTLP_ENDPOINT"),
            shutdown_deadline: seconds(get, "SHUTDOWN_DEADLINE_SECS", 30)?,
        })
    }
}
