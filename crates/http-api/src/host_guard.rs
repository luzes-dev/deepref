//! DNS-rebinding defence: reject requests addressed to a host the API does not
//! serve.
//!
//! A page on `evil.example` can rebind its name to `127.0.0.1` and then call the
//! API "same-origin", so CORS never applies. The browser still sends
//! `Host: evil.example`, which this layer refuses. Allowed hosts are the
//! loopback names, the bound address when it is a specific IP, and the hosts of
//! the configured `API_CORS_ORIGINS`.
//!
//! Ports are not compared: rebinding is defeated by the host name alone, and a
//! published container port or a same-origin proxy (Caddy forwards the
//! browser's `Host`) changes the port the API sees.

use std::{collections::BTreeSet, net::IpAddr, str::FromStr, sync::Arc};

use axum::{
    extract::{Request, State},
    http::{Uri, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{config::ApiConfig, error::ApiError};

/// Orchestrator probes address the container or pod IP, so they skip the check.
const PROBE_PATHS: [&str; 2] = ["/health/live", "/health/ready"];

#[derive(Debug, Clone)]
pub(crate) struct AllowedHosts(Arc<BTreeSet<String>>);

impl AllowedHosts {
    pub(crate) fn from_config(config: &ApiConfig) -> Self {
        let mut hosts: BTreeSet<String> = ["localhost", "127.0.0.1", "[::1]"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let bind_ip = config.bind_addr.ip();
        if !bind_ip.is_unspecified() {
            hosts.insert(ip_host(bind_ip));
        }
        for origin in &config.cors_origins {
            if let Some(host) = origin
                .to_str()
                .ok()
                .and_then(|value| value.parse::<Uri>().ok())
                .and_then(|uri| uri.host().map(normalize))
            {
                hosts.insert(host);
            }
        }
        Self(Arc::new(hosts))
    }

    fn allows(&self, authority: &str) -> bool {
        Authority::from_str(authority)
            .is_ok_and(|authority| self.0.contains(&normalize(authority.host())))
    }
}

fn ip_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

fn normalize(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Middleware: 403 for a request whose `Host` (or HTTP/2 `:authority`) is not
/// allowed. A request carrying neither cannot come from a browser and passes.
pub(crate) async fn reject_unknown_hosts(
    State(allowed): State<AllowedHosts>,
    request: Request,
    next: Next,
) -> Response {
    if PROBE_PATHS.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let authority = match request.headers().get(header::HOST) {
        Some(value) => value.to_str().ok().map(str::to_owned),
        None => request.uri().authority().map(ToString::to_string),
    };
    let allowed = match &authority {
        None if !request.headers().contains_key(header::HOST) => true,
        None => false,
        Some(authority) => allowed.allows(authority),
    };
    if allowed {
        next.run(request).await
    } else {
        tracing::warn!(
            host = authority.as_deref().unwrap_or("<invalid>"),
            "rejected request for a host outside the allow-list"
        );
        ApiError::Forbidden("request host is not allowed".to_owned()).into_response()
    }
}
