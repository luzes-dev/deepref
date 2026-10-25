//! Transport rollout configuration. PostgreSQL mode is the explicit rollback
//! path; JetStream changes delivery only, never durable execution authority.
use std::time::Duration;

use crate::{ConfigError, Environment, optional, parse};
use secrecy::SecretString;

#[derive(Debug, Clone, Default)]
pub enum WorkTransportConfig {
    #[default]
    Postgres,
    JetStream(NatsConfig),
}

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub url: String,
    pub token: Option<SecretString>,
    pub stream: String,
    pub subject_prefix: String,
    pub replicas: usize,
    pub max_bytes: i64,
    pub max_age: Duration,
    pub ack_wait: Duration,
    pub request_timeout: Duration,
    pub repair_after: Duration,
}

impl WorkTransportConfig {
    pub fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        match optional(get, "WORK_TRANSPORT")
            .as_deref()
            .unwrap_or("postgres")
        {
            "postgres" => Ok(Self::Postgres),
            "jetstream" => Ok(Self::JetStream(NatsConfig::from_source(environment, get)?)),
            _ => Err(invalid("WORK_TRANSPORT", "expected postgres or jetstream")),
        }
    }
}

impl NatsConfig {
    pub fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let url = optional(get, "NATS_URL")
            .ok_or_else(|| invalid("NATS_URL", "required for jetstream transport"))?;
        let parsed = url::Url::parse(&url)
            .map_err(|_| invalid("NATS_URL", "expected nats:// or tls:// server URL"))?;
        if !matches!(parsed.scheme(), "nats" | "tls")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !matches!(parsed.path(), "" | "/")
        {
            return Err(invalid(
                "NATS_URL",
                "expected a server URL without credentials, query or path; use NATS_TOKEN",
            ));
        }
        let token = optional(get, "NATS_TOKEN").map(SecretString::from);
        let production = !environment.is_development();
        let replicas = parse(get, "NATS_REPLICAS", if production { 3 } else { 1 })?;
        if !matches!(replicas, 1 | 3 | 5) || (production && replicas < 3) {
            return Err(invalid(
                "NATS_REPLICAS",
                "expected 1, 3 or 5; production requires at least 3",
            ));
        }
        if production && (parsed.scheme() != "tls" || token.is_none()) {
            return Err(invalid(
                "NATS_URL",
                "production requires TLS and NATS_TOKEN",
            ));
        }
        let stream = optional(get, "NATS_STREAM").unwrap_or_else(|| "DEEPREF_WORK".into());
        if stream.is_empty()
            || stream.len() > 64
            || !stream
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(invalid(
                "NATS_STREAM",
                "use 1-64 ASCII letters, digits or underscores",
            ));
        }
        let subject_prefix =
            optional(get, "NATS_SUBJECT_PREFIX").unwrap_or_else(|| "deepref".into());
        if subject_prefix.len() > 64
            || subject_prefix.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        {
            return Err(invalid(
                "NATS_SUBJECT_PREFIX",
                "expected bounded literal subject tokens",
            ));
        }
        let max_bytes = parse(get, "NATS_MAX_BYTES", 1_073_741_824_i64)?;
        let age = parse(get, "NATS_MAX_AGE_SECS", 604_800_u64)?;
        let ack = parse(get, "NATS_ACK_WAIT_SECS", 60_u64)?;
        let timeout = parse(get, "NATS_REQUEST_TIMEOUT_SECS", 5_u64)?;
        let repair = parse(get, "NATS_REPAIR_AFTER_SECS", 900_u64)?;
        for (name, valid) in [
            (
                "NATS_MAX_BYTES",
                (1_048_576..=1_099_511_627_776).contains(&max_bytes),
            ),
            ("NATS_MAX_AGE_SECS", (60..=2_592_000).contains(&age)),
            ("NATS_ACK_WAIT_SECS", (3..=3600).contains(&ack)),
            ("NATS_REQUEST_TIMEOUT_SECS", (1..=30).contains(&timeout)),
            ("NATS_REPAIR_AFTER_SECS", repair >= ack * 5 && repair <= age),
        ] {
            if !valid {
                return Err(invalid(
                    name,
                    "value is outside the bounded transport policy",
                ));
            }
        }
        Ok(Self {
            url,
            token,
            stream,
            subject_prefix,
            replicas,
            max_bytes,
            max_age: Duration::from_secs(age),
            ack_wait: Duration::from_secs(ack),
            request_timeout: Duration::from_secs(timeout),
            repair_after: Duration::from_secs(repair),
        })
    }
}

fn invalid(name: &'static str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        name,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(name: &str) -> Option<String> {
        match name {
            "WORK_TRANSPORT" => Some("jetstream".into()),
            "NATS_URL" => Some("nats://127.0.0.1:4222".into()),
            _ => None,
        }
    }
    #[test]
    fn rollout_and_production_validation() {
        assert!(matches!(
            WorkTransportConfig::from_source(Environment::Development, &|_| None).unwrap(),
            WorkTransportConfig::Postgres
        ));
        assert!(WorkTransportConfig::from_source(Environment::Development, &source).is_ok());
        assert!(WorkTransportConfig::from_source(Environment::Production, &source).is_err());
        assert!(
            NatsConfig::from_source(Environment::Production, &|key| match key {
                "NATS_URL" => Some("tls://nats.internal:4222".into()),
                "NATS_TOKEN" => Some("test-token".into()),
                _ => None,
            })
            .is_ok()
        );
        for (key, value) in [
            ("NATS_URL", "nats://secret:password@localhost"),
            ("NATS_REPLICAS", "2"),
            ("NATS_MAX_BYTES", "0"),
            ("NATS_ACK_WAIT_SECS", "0"),
            ("NATS_STREAM", "unsafe.*"),
            ("NATS_REPAIR_AFTER_SECS", "1"),
            ("NATS_REQUEST_TIMEOUT_SECS", "100"),
        ] {
            assert!(
                NatsConfig::from_source(Environment::Development, &|name| if name == key {
                    Some(value.into())
                } else {
                    source(name)
                })
                .is_err(),
                "{key}"
            );
        }
    }
}
