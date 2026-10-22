use std::fmt;

use secrecy::{ExposeSecret, SecretString};

use crate::{APP_ENV, ConfigError, Environment};

const KEY_NAME: &str = "DEEPREF_DELETION_DIGEST_KEY";
const MIN_KEY_BYTES: usize = 32;
const DEVELOPMENT_KEY: &str = "deepref-development-only-deletion-digest-key-not-for-production";

/// The secret used only by account-deletion completion and restore replay to
/// produce keyed, pseudonymous address digests. It is never sent to PostgreSQL.
#[derive(Clone)]
pub struct DeletionDigestConfig {
    key: SecretString,
}

impl DeletionDigestConfig {
    /// Loads the deletion digest key for the completion/replay command. A
    /// built-in key is available only for explicit development environments.
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let environment = Environment::from_app_env(get(APP_ENV).as_deref())?;
        let key = match crate::optional(get, KEY_NAME) {
            Some(key) => key,
            None if environment.is_development() => DEVELOPMENT_KEY.to_owned(),
            None => return Err(ConfigError::Missing(KEY_NAME, environment)),
        };
        if key.len() < MIN_KEY_BYTES {
            return Err(ConfigError::Invalid {
                name: KEY_NAME,
                reason: format!("must be at least {MIN_KEY_BYTES} bytes"),
            });
        }
        Ok(Self {
            key: SecretString::from(key),
        })
    }

    /// Exposes the secret to the account-deletion digest implementation.
    pub fn expose_secret(&self) -> &str {
        self.key.expose_secret()
    }
}

impl fmt::Debug for DeletionDigestConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeletionDigestConfig")
            .field("key", &"<redacted>")
            .finish()
    }
}
