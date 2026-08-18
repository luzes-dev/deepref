use std::time::Duration;

use crate::{ConfigError, Environment, database::seconds, optional};

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub url: String,
    pub credentials_file: Option<String>,
    pub ca_file: Option<String>,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub worker_consumer: String,
    pub projector_consumer: String,
}

impl NatsConfig {
    pub(crate) fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let url = optional(get, "NATS_URL")
            .or_else(|| {
                environment
                    .is_local()
                    .then(|| "nats://127.0.0.1:4222".into())
            })
            .ok_or(ConfigError::Missing("NATS_URL", environment))?;
        let credentials_file = optional(get, "NATS_CREDENTIALS_FILE");
        let ca_file = optional(get, "NATS_CA_FILE");
        if !environment.is_local() && credentials_file.is_none() {
            return Err(ConfigError::Missing("NATS_CREDENTIALS_FILE", environment));
        }
        if !environment.is_local() && ca_file.is_none() {
            return Err(ConfigError::Missing("NATS_CA_FILE", environment));
        }
        Ok(Self {
            url,
            credentials_file,
            ca_file,
            connect_timeout: seconds(get, "NATS_CONNECT_TIMEOUT_SECS", 5)?,
            request_timeout: seconds(get, "NATS_REQUEST_TIMEOUT_SECS", 10)?,
            worker_consumer: optional(get, "NATS_WORKER_CONSUMER")
                .unwrap_or_else(|| "deepref-worker".into()),
            projector_consumer: optional(get, "NATS_PROJECTOR_CONSUMER")
                .unwrap_or_else(|| "deepref-projector".into()),
        })
    }
}
