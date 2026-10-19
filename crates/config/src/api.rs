use std::net::SocketAddr;

use crate::{APP_ENV, ConfigError, Environment, ListenConfig};

#[derive(Debug, Clone)]
pub struct ApiSettings {
    pub bind_addr: SocketAddr,
    pub cors_allow_any: bool,
    pub cors_origins: Vec<String>,
}

impl ApiSettings {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let environment = Environment::from_app_env(get(APP_ENV).as_deref())?;
        let bind_addr = ListenConfig::from_source(get)?.bind_addr;
        let cors_allow_any = get("API_CORS_ALLOW_ANY").is_some_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        });
        if cors_allow_any && !environment.is_development() {
            return Err(ConfigError::Invalid {
                name: "API_CORS_ALLOW_ANY",
                reason: "permitted only when APP_ENV=development".into(),
            });
        }
        let raw_origins = get("API_CORS_ORIGINS").or_else(|| environment.is_development().then(||
            "http://localhost:3000,http://127.0.0.1:3000,http://localhost:5173,http://127.0.0.1:5173,http://localhost:4173,http://127.0.0.1:4173".into()
        )).unwrap_or_default();
        let cors_origins: Vec<_> = raw_origins
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        if !cors_allow_any && cors_origins.is_empty() && !environment.is_development() {
            return Err(ConfigError::Invalid {
                name: "API_CORS_ORIGINS",
                reason: "must contain at least one exact hosted origin".into(),
            });
        }
        Ok(Self {
            bind_addr,
            cors_allow_any,
            cors_origins,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cors_defaults_are_development_only() {
        let development =
            ApiSettings::from_source(&|name| (name == APP_ENV).then(|| "development".into()))
                .unwrap();
        assert_eq!(development.cors_origins.len(), 6);
        assert!(!development.cors_allow_any);
        assert!(ApiSettings::from_source(&|_| None).is_err());
        assert!(
            ApiSettings::from_source(&|name| (name == APP_ENV).then(|| "production".into()))
                .is_err()
        );
        assert!(
            ApiSettings::from_source(&|name| match name {
                APP_ENV => Some("production".into()),
                "API_CORS_ALLOW_ANY" => Some("true".into()),
                _ => None,
            })
            .is_err()
        );
    }
}
