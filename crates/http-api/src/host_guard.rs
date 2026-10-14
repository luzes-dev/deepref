//! DNS-rebinding defence: reject requests addressed to a host the API does not
//! serve.
//!
//! A page on `evil.example` can rebind its name to `127.0.0.1` and then call the
//! API "same-origin", so CORS never applies. The browser still sends
//! `Host: evil.example`, which this layer refuses. Allowed hosts are the
//! loopback names, the bound address when it is a specific IP, and the hosts of
//! the configured `API_CORS_ORIGINS`.
//!
//! Authorities include ports: the bind port and configured browser origins are
//! allowed. A proxy or published container port must be listed in
//! `API_CORS_ORIGINS`, since Caddy preserves the browser's Host.

use std::{collections::BTreeSet, net::IpAddr, str::FromStr, sync::Arc};

use axum::{
    extract::{Request, State},
    http::{Uri, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{config::ApiConfig, error::ApiError};

#[derive(Debug, Clone)]
pub(crate) struct AllowedHosts(Arc<BTreeSet<(String, u16)>>);

impl AllowedHosts {
    pub(crate) fn from_config(config: &ApiConfig) -> Self {
        let port = config.bind_addr.port();
        let mut hosts: BTreeSet<(String, u16)> = ["localhost", "127.0.0.1", "[::1]"]
            .into_iter()
            .map(|host| (host.to_owned(), port))
            .collect();
        let bind_ip = config.bind_addr.ip();
        if !bind_ip.is_unspecified() {
            hosts.insert((ip_host(bind_ip), port));
        }
        for origin in &config.cors_origins {
            if let Some(uri) = origin
                .to_str()
                .ok()
                .and_then(|value| value.parse::<Uri>().ok())
            {
                if let Some(host) = uri.host() {
                    let port = uri.port_u16().unwrap_or_else(|| {
                        if uri.scheme_str() == Some("https") {
                            443
                        } else {
                            80
                        }
                    });
                    hosts.insert((normalize(host), port));
                }
            }
        }
        Self(Arc::new(hosts))
    }

    fn allows(&self, authority: &str) -> bool {
        Authority::from_str(authority).is_ok_and(|authority| {
            let host = normalize(authority.host());
            match authority.port_u16() {
                Some(port) => self.0.contains(&(host, port)),
                None => self
                    .0
                    .iter()
                    .any(|(allowed_host, port)| allowed_host == &host && matches!(port, 80 | 443)),
            }
        })
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

/// Middleware: 403 for an unlisted `Host` (or HTTP/2 `:authority`).
pub(crate) async fn reject_unknown_hosts(
    State(allowed): State<AllowedHosts>,
    request: Request,
    next: Next,
) -> Response {
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
