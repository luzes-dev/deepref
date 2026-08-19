use std::net::SocketAddr;

use axum::http::HeaderValue;
use deepref_config::RuntimeConfig;

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub runtime: RuntimeConfig,
    pub bind_addr: SocketAddr,
    pub cors_allow_any: bool,
    pub cors_origins: Vec<HeaderValue>,
}

impl ApiConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let runtime = RuntimeConfig::from_env("deepref-api")?;
        let bind_addr = std::env::var("API_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()?;
        let cors_allow_any = bool_env("API_CORS_ALLOW_ANY");
        if cors_allow_any && !runtime.environment.is_local() {
            anyhow::bail!("API_CORS_ALLOW_ANY is permitted only when APP_ENV=local");
        }
        let raw_origins = std::env::var("API_CORS_ORIGINS").ok().or_else(|| runtime.environment.is_local().then(||
            "http://localhost:3000,http://127.0.0.1:3000,http://localhost:5173,http://127.0.0.1:5173".into()
        )).unwrap_or_default();
        let cors_origins = raw_origins
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                HeaderValue::from_str(value).map_err(|error| {
                    anyhow::anyhow!("invalid API_CORS_ORIGINS entry {value}: {error}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !cors_allow_any && cors_origins.is_empty() && !runtime.environment.is_local() {
            anyhow::bail!("API_CORS_ORIGINS must contain at least one exact hosted origin");
        }
        Ok(Self {
            runtime,
            bind_addr,
            cors_allow_any,
            cors_origins,
        })
    }
}

fn bool_env(name: &str) -> bool {
    std::env::var(name)
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiCommand {
    Serve,
    Migrate,
    ImportLegacy,
    PrintOpenApi,
}

impl ApiCommand {
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();
        match arguments.as_slice() {
            [] => Ok(Self::Serve),
            [value] if value == "serve" => Ok(Self::Serve),
            [value] if value == "migrate" => Ok(Self::Migrate),
            [value] if value == "import-legacy" => Ok(Self::ImportLegacy),
            [value] if value == "--print-openapi" => Ok(Self::PrintOpenApi),
            _ => anyhow::bail!("usage: deepref-api [serve|migrate|import-legacy|--print-openapi]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_contract_is_explicit() {
        assert_eq!(
            ApiCommand::parse(["migrate".into()]).unwrap(),
            ApiCommand::Migrate
        );
        assert_eq!(
            ApiCommand::parse(["import-legacy".into()]).unwrap(),
            ApiCommand::ImportLegacy
        );
        assert!(ApiCommand::parse(["serve".into(), "migrate".into()]).is_err());
    }
}
