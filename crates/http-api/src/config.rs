use std::net::SocketAddr;

use axum::http::HeaderValue;
use deepref_config::{ListenConfig, RuntimeConfig};

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
        // Loopback by default; a network bind needs DEEPREF_ALLOW_NETWORK_BIND.
        let bind_addr = ListenConfig::from_env()?.bind_addr;
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
