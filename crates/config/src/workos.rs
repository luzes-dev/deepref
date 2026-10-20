use secrecy::{ExposeSecret, SecretString};

use crate::{ConfigError, Environment};

const CLIENT_ID: &str = "WORKOS_CLIENT_ID";
const API_KEY: &str = "WORKOS_API_KEY";
const COOKIE_PASSWORD: &str = "WORKOS_COOKIE_PASSWORD";
const WEBHOOK_SECRET: &str = "WORKOS_WEBHOOK_SECRET";
/// WorkOS rejects a sealed-session password shorter than this.
const MIN_COOKIE_PASSWORD_BYTES: usize = 32;

/// WorkOS settings. The section is all or nothing: every variable is set, or
/// none is. Secrets are never printed: `Debug` redacts them and each has one
/// accessor. Nothing consumes this yet.
#[derive(Debug, Clone)]
pub struct WorkOsConfig {
    pub client_id: String,
    api_key: SecretString,
    cookie_password: SecretString,
    webhook_secret: SecretString,
}

impl WorkOsConfig {
    pub(crate) fn from_source(
        environment: Environment,
        auth_enabled: bool,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, ConfigError> {
        let values = crate::section(
            "WorkOS",
            get,
            [CLIENT_ID, API_KEY, COOKIE_PASSWORD, WEBHOOK_SECRET],
        )?;
        let Some([client_id, api_key, cookie_password, webhook_secret]) = values else {
            if auth_enabled && !environment.is_development() {
                return Err(ConfigError::Invalid {
                    name: crate::AUTH_ENABLED,
                    reason: format!(
                        "requires WorkOS when APP_ENV=production; set {CLIENT_ID}, {API_KEY}, \
                         {COOKIE_PASSWORD} and {WEBHOOK_SECRET}"
                    ),
                });
            }
            return Ok(None);
        };
        if cookie_password.len() < MIN_COOKIE_PASSWORD_BYTES {
            return Err(ConfigError::Invalid {
                name: COOKIE_PASSWORD,
                reason: format!("must be at least {MIN_COOKIE_PASSWORD_BYTES} characters"),
            });
        }
        Ok(Some(Self {
            client_id,
            api_key: SecretString::from(api_key),
            cookie_password: SecretString::from(cookie_password),
            webhook_secret: SecretString::from(webhook_secret),
        }))
    }

    pub fn api_key(&self) -> &str {
        self.api_key.expose_secret()
    }

    pub fn cookie_password(&self) -> &str {
        self.cookie_password.expose_secret()
    }

    pub fn webhook_secret(&self) -> &str {
        self.webhook_secret.expose_secret()
    }
}
