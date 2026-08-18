use serde::{Deserialize, Serialize};

use crate::ConfigError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Local,
    Development,
    Staging,
    Production,
}

impl Environment {
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "development" | "dev" => Ok(Self::Development),
            "staging" => Ok(Self::Staging),
            "production" | "prod" => Ok(Self::Production),
            value => Err(ConfigError::Invalid {
                name: "APP_ENV",
                reason: format!("unsupported environment {value:?}"),
            }),
        }
    }

    pub const fn is_local(self) -> bool {
        matches!(self, Self::Local)
    }
}
