use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// The variable that selects the runtime environment.
pub const APP_ENV: &str = "APP_ENV";

/// The dotenv files read in development, in precedence order.
const DEVELOPMENT_DOTENV_FILES: [&str; 2] = [".env.local", ".env"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Development,
    Production,
}

impl Environment {
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" => Ok(Self::Development),
            "production" | "prod" => Ok(Self::Production),
            "local" => Err(ConfigError::Invalid {
                name: APP_ENV,
                reason: "local was removed; use development".into(),
            }),
            "staging" => Err(ConfigError::Invalid {
                name: APP_ENV,
                reason: "staging is not a runtime environment; use production for any deployed \
                         service and development for a developer machine"
                    .into(),
            }),
            value => Err(ConfigError::Invalid {
                name: APP_ENV,
                reason: format!("unsupported environment {value:?}; use development or production"),
            }),
        }
    }

    /// Resolves `APP_ENV`. An unset or blank variable is a startup error: no
    /// default is safe for both a developer machine and a hosted deploy.
    pub fn from_app_env(value: Option<&str>) -> Result<Self, ConfigError> {
        match value.map(str::trim) {
            None | Some("") => Err(ConfigError::AppEnvMissing),
            Some(value) => Self::parse(value),
        }
    }

    pub const fn is_development(self) -> bool {
        matches!(self, Self::Development)
    }
}

/// Loads `.env.local` and then `.env` from the working directory (or any
/// ancestor), but only when the process environment already says
/// `APP_ENV=development`. Production never reads files from the working
/// directory. Variables already present in the process environment always win,
/// and because `.env.local` is read first it takes precedence over `.env`.
/// Missing files are not an error. Returns whether the files were considered.
pub fn load_development_dotenv_files() -> bool {
    let files = dotenv_files(std::env::var(APP_ENV).ok().as_deref());
    for file in files {
        let _ = dotenvy::from_filename(file);
    }
    !files.is_empty()
}

fn dotenv_files(app_env: Option<&str>) -> &'static [&'static str] {
    let development = app_env.is_some_and(|value| {
        Environment::from_app_env(Some(value)).is_ok_and(Environment::is_development)
    });
    if development {
        &DEVELOPMENT_DOTENV_FILES
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_blank_app_env_is_an_error_never_a_default() {
        assert!(matches!(
            Environment::from_app_env(None),
            Err(ConfigError::AppEnvMissing)
        ));
        assert!(matches!(
            Environment::from_app_env(Some("  ")),
            Err(ConfigError::AppEnvMissing)
        ));
    }

    #[test]
    fn only_development_and_production_are_accepted() {
        for (value, expected) in [
            ("development", Environment::Development),
            (" DEV ", Environment::Development),
            ("production", Environment::Production),
            ("Prod", Environment::Production),
        ] {
            assert_eq!(Environment::parse(value).unwrap(), expected);
        }
    }

    #[test]
    fn removed_environments_name_their_replacement() {
        let local = Environment::parse("local").unwrap_err().to_string();
        assert!(local.contains("use development"), "{local}");
        let staging = Environment::parse("staging").unwrap_err().to_string();
        assert!(staging.contains("use production"), "{staging}");
        assert!(Environment::parse("test").is_err());
    }

    #[test]
    fn dotenv_files_load_only_for_an_explicit_development_environment() {
        assert_eq!(dotenv_files(Some("development")), [".env.local", ".env"]);
        assert_eq!(dotenv_files(Some(" DEV ")), [".env.local", ".env"]);
        for other in [
            None,
            Some("production"),
            Some("prod"),
            Some("local"),
            Some("staging"),
            Some("bogus"),
            Some(""),
        ] {
            assert!(dotenv_files(other).is_empty(), "{other:?}");
        }
    }
}
