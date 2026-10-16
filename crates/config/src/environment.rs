use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// The variable that selects the deployment environment.
pub const APP_ENV: &str = "APP_ENV";

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
                name: APP_ENV,
                reason: format!("unsupported environment {value:?}"),
            }),
        }
    }

    /// Resolves `APP_ENV`. An unset variable means `production`, so a deploy
    /// that forgets it keeps every hosted fail-fast guard. Local development
    /// opts in with `APP_ENV=local` (`.env.example`, `just`, Process Compose).
    pub fn from_app_env(value: Option<&str>) -> Result<Self, ConfigError> {
        value.map_or(Ok(Self::Production), Self::parse)
    }

    pub const fn is_local(self) -> bool {
        matches!(self, Self::Local)
    }
}

/// Loads `.env.local` and then `.env` from the working directory (or any
/// ancestor), but only when the process environment already says
/// `APP_ENV=local`. Hosted environments never read files from the working
/// directory. Variables already present in the process environment always win,
/// and because `.env.local` is read first it takes precedence over `.env`.
/// Missing files are not an error. Returns whether the files were considered.
pub fn load_local_dotenv_files() -> bool {
    let load = dotenv_allowed(std::env::var(APP_ENV).ok().as_deref());
    if load {
        let _ = dotenvy::from_filename(".env.local");
        let _ = dotenvy::from_filename(".env");
    }
    load
}

fn dotenv_allowed(app_env: Option<&str>) -> bool {
    app_env.is_some_and(|value| Environment::parse(value).is_ok_and(Environment::is_local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_app_env_defaults_to_production() {
        assert!(matches!(
            Environment::from_app_env(None),
            Ok(Environment::Production)
        ));
    }

    #[test]
    fn dotenv_files_load_only_for_an_explicit_local_environment() {
        assert!(dotenv_allowed(Some("local")));
        assert!(dotenv_allowed(Some(" LOCAL ")));
        assert!(!dotenv_allowed(None));
        assert!(!dotenv_allowed(Some("production")));
        assert!(!dotenv_allowed(Some("development")));
        assert!(!dotenv_allowed(Some("bogus")));
    }
}
