//! Secret-safe identity of the provider endpoint that answers AI calls.
//!
//! A model name does not identify what answered a call: the same name can be
//! served by another backend when the base URL changes. Review identity and
//! audit therefore need the endpoint. The configured URL, however, can carry
//! credentials (`https://user:key@host/...`) and request-level query state.
//! [`ProviderEndpoint`] keeps only the part that selects the API, so it is safe
//! to persist in manifests, log and compare.
//!
//! Each process registers the endpoint of the gateway it builds for a provider
//! ([`register_provider_endpoint`]). Manifests record the endpoint of the
//! process that scheduled them, and a worker refuses to execute a route whose
//! recorded endpoint is not the one it calls. The registry is process-wide
//! because the scheduler and the executor meet only through configuration;
//! persisting the endpoint on route rows would make the last process to start
//! decide it for every process sharing the database.

use std::{collections::BTreeMap, fmt, sync::RwLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::AiError;

/// The normalized `scheme://host[:port][/path]` of a provider endpoint, for
/// example `https://opencode.ai/zen/go/v1`.
///
/// Kept: the scheme, the lowercase host (IDNA as the `url` crate gives it), the
/// port when it is not the scheme default, and the path without trailing
/// slashes. Dropped: the username and password, the query and the fragment.
///
/// Path segments are kept because they select the API: `/v1` and `/v2` can be
/// different services. For the same reason, a secret must never be placed in
/// the path, because the path is stored in review manifests. Keys belong in the
/// API key setting, which this type never sees.
///
/// `Debug` and `Display` print only the normalized form, and errors never echo
/// the configured URL.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderEndpoint(String);

/// A configured URL that cannot identify an endpoint. The message omits the
/// URL on purpose: it may contain credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProviderEndpointError {
    #[error("provider endpoint must be an absolute http or https URL with a host")]
    Invalid,
}

impl ProviderEndpoint {
    /// Normalizes a configured base URL. Only `http` and `https` URLs with a
    /// host are accepted.
    pub fn from_configured_url(raw: &str) -> Result<Self, ProviderEndpointError> {
        let url = url::Url::parse(raw).map_err(|_| ProviderEndpointError::Invalid)?;
        let scheme = url.scheme();
        if !matches!(scheme, "http" | "https") {
            return Err(ProviderEndpointError::Invalid);
        }
        let host = url
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or(ProviderEndpointError::Invalid)?;
        // The `url` crate already leaves out a port that is the scheme default.
        let port = url
            .port()
            .map_or_else(String::new, |port| format!(":{port}"));
        let path = url.path().trim_end_matches('/');
        Ok(Self(format!("{scheme}://{host}{port}{path}")))
    }

    /// The normalized endpoint.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProviderEndpoint {
    type Error = ProviderEndpointError;

    /// Accepts only the normalized form, so a stored value cannot carry
    /// anything the normalizer would have removed.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let endpoint = Self::from_configured_url(&value)?;
        if endpoint.0 == value {
            Ok(endpoint)
        } else {
            Err(ProviderEndpointError::Invalid)
        }
    }
}

impl From<ProviderEndpoint> for String {
    fn from(endpoint: ProviderEndpoint) -> Self {
        endpoint.0
    }
}

impl fmt::Display for ProviderEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for ProviderEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ProviderEndpoint")
            .field(&self.0)
            .finish()
    }
}

/// Endpoint of the gateway this process built for each provider id.
static PROVIDER_ENDPOINTS: RwLock<BTreeMap<String, ProviderEndpoint>> =
    RwLock::new(BTreeMap::new());

fn registry_error() -> AiError {
    AiError::Gateway("provider endpoint registry is unavailable".to_owned())
}

/// Records the endpoint that this process's gateway for `provider` calls.
///
/// Registering the same endpoint again is a no-op, because the API and the
/// worker can start in one process (`apps/server`). A different endpoint for a
/// provider that is already registered is refused: one process must not hold
/// two answers for the same identity.
pub fn register_provider_endpoint(
    provider: &str,
    endpoint: ProviderEndpoint,
) -> Result<(), AiError> {
    let mut endpoints = PROVIDER_ENDPOINTS.write().map_err(|_| registry_error())?;
    match endpoints.get(provider) {
        Some(existing) if *existing == endpoint => Ok(()),
        Some(_) => Err(AiError::Gateway(
            "provider is already bound to another endpoint in this process".to_owned(),
        )),
        None => {
            endpoints.insert(provider.to_owned(), endpoint);
            Ok(())
        }
    }
}

/// The endpoint this process calls for `provider`, or `None` when no gateway
/// for that provider has been built here.
pub fn provider_endpoint(provider: &str) -> Result<Option<ProviderEndpoint>, AiError> {
    let endpoints = PROVIDER_ENDPOINTS.read().map_err(|_| registry_error())?;
    Ok(endpoints.get(provider).cloned())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use deepref_domain::ProjectId;

    use super::*;
    use crate::{
        AiFuture, BudgetSnapshot, PriceBook, UsageEntry, UsageLedger, build_metered_provider,
    };

    fn normalized(raw: &str) -> String {
        ProviderEndpoint::from_configured_url(raw)
            .unwrap_or_else(|_| unreachable!("{raw} should normalize"))
            .as_str()
            .to_owned()
    }

    #[test]
    fn userinfo_query_and_fragment_are_dropped() {
        assert_eq!(
            normalized("https://user:secret@Host.Example:443/v1/?key=abc#frag"),
            "https://host.example/v1"
        );
    }

    #[test]
    fn default_ports_are_dropped_and_other_ports_are_kept() {
        assert_eq!(
            normalized("https://host.example:443/v1"),
            "https://host.example/v1"
        );
        assert_eq!(normalized("http://host.example:80"), "http://host.example");
        assert_eq!(
            normalized("https://host.example:8443/v1"),
            "https://host.example:8443/v1"
        );
        assert_eq!(
            normalized("http://host.example:443"),
            "http://host.example:443"
        );
    }

    #[test]
    fn trailing_slashes_are_removed_and_nothing_else_is_collapsed() {
        assert_eq!(normalized("https://host.example/"), "https://host.example");
        assert_eq!(
            normalized("https://host.example/v1//"),
            "https://host.example/v1"
        );
        assert_eq!(
            normalized("https://host.example//v1"),
            "https://host.example//v1"
        );
    }

    #[test]
    fn scheme_and_host_are_lowercased() {
        assert_eq!(
            normalized("HTTPS://OpenCode.AI/zen/go/v1"),
            "https://opencode.ai/zen/go/v1"
        );
    }

    #[test]
    fn ipv6_hosts_keep_their_brackets_and_port() {
        assert_eq!(normalized("http://[::1]:8080/v1/"), "http://[::1]:8080/v1");
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        for raw in [
            "ftp://host.example/v1",
            "file:///etc/hosts",
            "ws://host.example",
            "javascript:alert(1)",
        ] {
            assert_eq!(
                ProviderEndpoint::from_configured_url(raw),
                Err(ProviderEndpointError::Invalid),
                "{raw}"
            );
        }
    }

    #[test]
    fn garbage_and_hostless_urls_are_rejected() {
        for raw in ["", "not a url", "https://", "https:///", "http://:8080/v1"] {
            assert_eq!(
                ProviderEndpoint::from_configured_url(raw),
                Err(ProviderEndpointError::Invalid),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn errors_and_debug_output_never_echo_the_configured_url() {
        let error =
            ProviderEndpoint::from_configured_url("ftp://user:secret@host.example/v1?key=abc")
                .err()
                .map(|error| format!("{error} {error:?}"))
                .unwrap_or_default();
        assert!(!error.is_empty());
        assert!(!error.contains("secret"), "{error}");
        assert!(!error.contains("abc"), "{error}");

        let endpoint =
            ProviderEndpoint::from_configured_url("https://user:secret@host.example/v1?key=abc")
                .unwrap_or_else(|_| unreachable!());
        let rendered = format!("{endpoint} {endpoint:?}");
        assert!(rendered.contains("https://host.example/v1"), "{rendered}");
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(!rendered.contains("abc"), "{rendered}");
    }

    #[test]
    fn identical_routes_normalize_to_equal_endpoints() {
        let plain = ProviderEndpoint::from_configured_url("https://opencode.ai/zen/go/v1")
            .unwrap_or_else(|_| unreachable!());
        let decorated = ProviderEndpoint::from_configured_url(
            "https://user:key@OPENCODE.ai:443/zen/go/v1/?x=1#y",
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(plain, decorated);
        let other_path = ProviderEndpoint::from_configured_url("https://opencode.ai/zen/v1")
            .unwrap_or_else(|_| unreachable!());
        assert_ne!(plain, other_path);
    }

    #[test]
    fn serialized_form_is_the_normalized_string_and_stored_values_must_already_be_normal() {
        let endpoint = ProviderEndpoint::from_configured_url("https://opencode.ai/zen/go/v1/")
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            serde_json::to_string(&endpoint).unwrap_or_default(),
            "\"https://opencode.ai/zen/go/v1\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderEndpoint>("\"https://opencode.ai/zen/go/v1\"")
                .ok()
                .as_ref(),
            Some(&endpoint)
        );
        assert!(
            serde_json::from_str::<ProviderEndpoint>("\"https://opencode.ai/zen/go/v1/\"").is_err()
        );
        assert!(
            serde_json::from_str::<ProviderEndpoint>("\"https://user:secret@host.example/\"")
                .is_err()
        );
    }

    #[test]
    fn the_registry_returns_what_was_registered_and_refuses_a_different_endpoint() {
        let provider = "registry-unit-test";
        assert_eq!(provider_endpoint(provider).ok().flatten(), None);
        let first = ProviderEndpoint::from_configured_url("https://first.example/v1")
            .unwrap_or_else(|_| unreachable!());
        register_provider_endpoint(provider, first.clone()).unwrap_or_else(|_| unreachable!());
        register_provider_endpoint(provider, first.clone()).unwrap_or_else(|_| unreachable!());
        assert_eq!(
            provider_endpoint(provider).ok().flatten(),
            Some(first.clone())
        );
        let other = ProviderEndpoint::from_configured_url("https://second.example/v1")
            .unwrap_or_else(|_| unreachable!());
        assert!(register_provider_endpoint(provider, other).is_err());
        assert_eq!(provider_endpoint(provider).ok().flatten(), Some(first));
    }

    /// A ledger that never refuses a call. The wiring test never makes one.
    struct OpenLedger;

    impl UsageLedger for OpenLedger {
        fn budget<'a>(&'a self, _project_id: ProjectId) -> AiFuture<'a, BudgetSnapshot> {
            Box::pin(async {
                Ok(BudgetSnapshot {
                    budget_micros: i64::MAX,
                    spent_micros: 0,
                })
            })
        }

        fn record<'a>(&'a self, _entry: UsageEntry) -> AiFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn building_the_metered_gateway_registers_the_endpoint_it_calls() {
        let built = build_metered_provider(
            "zai",
            "https://user:secret@Gateway.Example:443/v1/?key=abc",
            "test-key",
            Arc::new(OpenLedger),
            PriceBook::new(Vec::new()),
        );
        assert!(built.is_ok());
        let registered = provider_endpoint("zai").ok().flatten();
        assert_eq!(
            registered.map(|endpoint| endpoint.to_string()),
            Some("https://gateway.example/v1".to_owned())
        );

        // Building again with the same endpoint is fine: the API and the worker can share a
        // process. A different endpoint for the same provider is refused.
        assert!(
            build_metered_provider(
                "zai",
                "https://gateway.example/v1/",
                "test-key",
                Arc::new(OpenLedger),
                PriceBook::new(Vec::new()),
            )
            .is_ok()
        );
        assert!(
            build_metered_provider(
                "zai",
                "https://elsewhere.example/v1",
                "test-key",
                Arc::new(OpenLedger),
                PriceBook::new(Vec::new()),
            )
            .is_err()
        );
        let still = provider_endpoint("zai").ok().flatten();
        assert_eq!(
            still.map(|endpoint| endpoint.to_string()),
            Some("https://gateway.example/v1".to_owned())
        );
    }
}
