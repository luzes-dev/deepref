use axum::http::HeaderValue;
use std::env;
use tower_http::cors::AllowOrigin;

pub(crate) struct ApiConfig {
    pub(crate) database_url: String,
    pub(crate) nats_url: String,
    pub(crate) bind_addr: String,
}

impl ApiConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://deepref:deepref@localhost:5432/deepref".to_owned()),
            nats_url: env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_owned()),
            bind_addr: env::var("API_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned()),
        }
    }
}

pub(crate) fn cors_origins() -> AllowOrigin {
    let allow_any = env::var("API_CORS_ALLOW_ANY")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);

    if allow_any {
        tracing::warn!("API_CORS_ALLOW_ANY is enabled; use only for local testing");
        return AllowOrigin::any();
    }

    let origins = env::var("API_CORS_ORIGINS").unwrap_or_else(|_| {
        [
            "http://localhost:3000",
            "http://127.0.0.1:3000",
            "http://localhost:5173",
            "http://127.0.0.1:5173",
        ]
        .join(",")
    });

    let origins = origins
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .filter_map(|origin| match origin.parse::<HeaderValue>() {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(%origin, %error, "ignoring invalid CORS origin");
                None
            }
        })
        .collect::<Vec<_>>();

    AllowOrigin::list(origins)
}
