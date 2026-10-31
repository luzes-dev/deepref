use std::time::Duration;

use crate::{ConfigError, Environment, SecretString};

/// Optional acceleration, never an independent authorization/quota authority.
#[derive(Clone)]
pub struct ValkeyConfig {
    pub url: SecretString,
    pub namespace: String,
    pub timeout: Duration,
}

impl std::fmt::Debug for ValkeyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyConfig")
            .field("url", &"[REDACTED]")
            .field("namespace", &self.namespace)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ValkeyConfig {
    pub(crate) fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, ConfigError> {
        let Some([address, namespace]) = crate::section(
            "Valkey",
            get,
            ["DEEPREF_VALKEY_URL", "DEEPREF_VALKEY_NAMESPACE"],
        )?
        else {
            if crate::optional(get, "DEEPREF_VALKEY_TIMEOUT_MS").is_some() {
                return Err(invalid(
                    "DEEPREF_VALKEY_TIMEOUT_MS",
                    "requires a configured Valkey endpoint",
                ));
            }
            return Ok(None);
        };
        let parsed = url::Url::parse(&address)
            .map_err(|_| invalid("DEEPREF_VALKEY_URL", "must be a valid Valkey URL"))?;
        let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let secure = parsed.scheme() == "rediss";
        if !matches!(parsed.scheme(), "redis" | "rediss")
            || parsed.host_str().is_none()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !matches!(parsed.path(), "" | "/" | "/0")
        {
            return Err(invalid(
                "DEEPREF_VALKEY_URL",
                "requires redis/rediss, database 0, and no query or fragment",
            ));
        }
        if (!environment.is_development() || !local)
            && (!secure || parsed.password().is_none_or(str::is_empty))
        {
            return Err(invalid(
                "DEEPREF_VALKEY_URL",
                "deployed endpoints require TLS and a password",
            ));
        }
        if namespace.is_empty()
            || namespace.len() > 64
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid(
                "DEEPREF_VALKEY_NAMESPACE",
                "use 1..64 lowercase letters, digits or hyphens, unique to this deployment",
            ));
        }
        let millis: u64 = crate::parse(get, "DEEPREF_VALKEY_TIMEOUT_MS", 100)?;
        if !(10..=500).contains(&millis) {
            return Err(invalid(
                "DEEPREF_VALKEY_TIMEOUT_MS",
                "must be between 10 and 500",
            ));
        }
        Ok(Some(Self {
            url: address.into(),
            namespace,
            timeout: Duration::from_millis(millis),
        }))
    }
}

fn invalid(name: &'static str, reason: &'static str) -> ConfigError {
    ConfigError::Invalid {
        name,
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(
        env: Environment,
        values: &[(&str, &str)],
    ) -> Result<Option<ValkeyConfig>, ConfigError> {
        ValkeyConfig::from_source(env, &|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, v)| (*v).to_owned())
        })
    }

    #[test]
    fn optional_endpoint_is_validated_without_echoing_secrets() {
        assert!(
            load(Environment::Development, &[])
                .expect("defaults")
                .is_none()
        );
        assert!(
            load(
                Environment::Development,
                &[("DEEPREF_VALKEY_NAMESPACE", "test")]
            )
            .is_err()
        );
        let config = load(
            Environment::Development,
            &[
                ("DEEPREF_VALKEY_NAMESPACE", "test"),
                ("DEEPREF_VALKEY_URL", "redis://:secret@127.0.0.1:6379"),
            ],
        )
        .expect("local")
        .expect("configured");
        assert!(!format!("{config:?}").contains("secret"));
        for url in [
            "redis://:secret@example.org",
            "rediss://example.org",
            "rediss://:secret@example.org/1",
            "redis://:secret@example.org?password=secret",
        ] {
            let error = load(
                Environment::Production,
                &[
                    ("DEEPREF_VALKEY_NAMESPACE", "test"),
                    ("DEEPREF_VALKEY_URL", url),
                ],
            )
            .expect_err("unsafe");
            assert!(!format!("{error:?}").contains("secret"));
        }
        assert!(
            load(
                Environment::Production,
                &[
                    ("DEEPREF_VALKEY_NAMESPACE", "test"),
                    ("DEEPREF_VALKEY_URL", "rediss://:secret@example.org")
                ]
            )
            .is_ok()
        );
    }
}
