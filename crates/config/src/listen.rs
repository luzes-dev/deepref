use std::{collections::HashMap, net::SocketAddr};

use crate::{ConfigError, optional};

pub const API_BIND_ADDR: &str = "API_BIND_ADDR";
/// Opt-in for listening on anything other than a loopback address.
///
/// The API has no authentication, so set this only when the API sits behind
/// an authenticating proxy (or inside a container network that only such a
/// proxy can reach).
pub const ALLOW_NETWORK_BIND: &str = "DEEPREF_ALLOW_NETWORK_BIND";
pub const DEFAULT_API_BIND_ADDR: &str = "127.0.0.1:8080";

/// Where the HTTP API listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenConfig {
    pub bind_addr: SocketAddr,
    /// The operator explicitly allowed a non-loopback bind.
    pub allow_network_bind: bool,
}

impl ListenConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(&|name| std::env::var(name).ok())
    }

    pub fn from_map(values: &HashMap<String, String>) -> Result<Self, ConfigError> {
        Self::from_source(&|name| values.get(name).cloned())
    }

    fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let raw = optional(get, API_BIND_ADDR).unwrap_or_else(|| DEFAULT_API_BIND_ADDR.to_owned());
        let bind_addr = raw
            .parse::<SocketAddr>()
            .map_err(|error| ConfigError::Invalid {
                name: API_BIND_ADDR,
                reason: format!("{raw:?} is not a socket address: {error}"),
            })?;
        let allow_network_bind = flag(get, ALLOW_NETWORK_BIND)?;
        if !bind_addr.ip().is_loopback() && !allow_network_bind {
            return Err(ConfigError::Invalid {
                name: API_BIND_ADDR,
                reason: format!(
                    "{bind_addr} is not a loopback address and the API has no authentication; \
                     set {ALLOW_NETWORK_BIND}=1 only when an authenticating proxy is the sole \
                     route to it"
                ),
            });
        }
        Ok(Self {
            bind_addr,
            allow_network_bind,
        })
    }
}

fn flag(get: &dyn Fn(&str) -> Option<String>, name: &'static str) -> Result<bool, ConfigError> {
    match optional(get, name)
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("0" | "false" | "no") => Ok(false),
        Some("1" | "true" | "yes") => Ok(true),
        Some(value) => Err(ConfigError::Invalid {
            name,
            reason: format!("{value:?} is not one of 1, true, yes, 0, false, no"),
        }),
    }
}
