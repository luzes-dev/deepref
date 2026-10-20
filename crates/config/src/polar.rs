use secrecy::{ExposeSecret, SecretString};

use crate::{ConfigError, Environment};

const ACCESS_TOKEN: &str = "POLAR_ACCESS_TOKEN";
const WEBHOOK_SECRET: &str = "POLAR_WEBHOOK_SECRET";
const SERVER: &str = "POLAR_SERVER";

/// Which Polar deployment the access token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolarServer {
    Sandbox,
    Production,
}

impl PolarServer {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Production => "production",
        }
    }

    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.to_ascii_lowercase().as_str() {
            "sandbox" => Ok(Self::Sandbox),
            "production" => Ok(Self::Production),
            _ => Err(ConfigError::Invalid {
                name: SERVER,
                reason: "must be sandbox or production".into(),
            }),
        }
    }
}

/// Polar settings. The section is all or nothing, and the server is always
/// explicit so a sandbox token cannot be mistaken for a live one. Secrets are
/// never printed: `Debug` redacts them and each has one accessor. Nothing
/// consumes this yet.
#[derive(Debug, Clone)]
pub struct PolarConfig {
    pub server: PolarServer,
    access_token: SecretString,
    webhook_secret: SecretString,
}

impl PolarConfig {
    pub(crate) fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, ConfigError> {
        let Some([access_token, webhook_secret, server]) =
            crate::section("Polar", get, [ACCESS_TOKEN, WEBHOOK_SECRET, SERVER])?
        else {
            return Ok(None);
        };
        let server = PolarServer::parse(&server)?;
        if !environment.is_development() && server != PolarServer::Production {
            return Err(ConfigError::Invalid {
                name: SERVER,
                reason: "must be production when APP_ENV=production".into(),
            });
        }
        Ok(Some(Self {
            server,
            access_token: SecretString::from(access_token),
            webhook_secret: SecretString::from(webhook_secret),
        }))
    }

    pub fn access_token(&self) -> &str {
        self.access_token.expose_secret()
    }

    pub fn webhook_secret(&self) -> &str {
        self.webhook_secret.expose_secret()
    }
}
