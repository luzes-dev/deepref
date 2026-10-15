use std::net::SocketAddr;

use axum::http::HeaderValue;
use deepref_config::{ApiSettings, DocumentConfig, RuntimeConfig};

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub runtime: RuntimeConfig,
    pub documents: DocumentConfig,
    pub bind_addr: SocketAddr,
    pub cors_allow_any: bool,
    pub cors_origins: Vec<HeaderValue>,
}

impl ApiConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let runtime = RuntimeConfig::from_source("deepref-api", get)?;
        let settings = ApiSettings::from_source(get)?;
        let documents = DocumentConfig::from_source(get)?;
        let cors_origins = settings
            .cors_origins
            .iter()
            .map(|value| {
                HeaderValue::from_str(value).map_err(|error| {
                    anyhow::anyhow!("invalid API_CORS_ORIGINS entry {value}: {error}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            runtime,
            documents,
            bind_addr: settings.bind_addr,
            cors_allow_any: settings.cors_allow_any,
            cors_origins,
        })
    }
}
