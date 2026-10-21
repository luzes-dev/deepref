use std::fmt;

use url::Url;

use crate::{ConfigError, Environment, optional};

/// The variable that names the canonical public origin of the application.
pub const PUBLIC_ORIGIN: &str = "DEEPREF_PUBLIC_ORIGIN";
/// The web dev server started by `just up` and Process Compose.
pub const DEFAULT_DEVELOPMENT_PUBLIC_ORIGIN: &str = "http://127.0.0.1:5173";

/// The canonical origin users reach the application at: scheme, host and
/// optional port, with no path, query, fragment or credentials. Cookies,
/// redirects and webhooks derive their URLs from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigin {
    serialized: String,
    https: bool,
}

impl PublicOrigin {
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        let invalid = |reason: &str| ConfigError::Invalid {
            name: PUBLIC_ORIGIN,
            reason: reason.to_owned(),
        };
        let url = Url::parse(raw.trim()).map_err(|_| {
            invalid(
                "must be an origin such as https://app.example.com (scheme, host, optional port)",
            )
        })?;
        let https = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(invalid("must use the http or https scheme")),
        };
        if url.host_str().is_none() {
            return Err(invalid("must include a host"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(invalid("must not contain credentials"));
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(invalid("must not contain a path, query or fragment"));
        }
        Ok(Self {
            serialized: url.origin().ascii_serialization(),
            https,
        })
    }

    pub(crate) fn from_source(
        environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let origin = match optional(get, PUBLIC_ORIGIN) {
            Some(raw) => Self::parse(&raw)?,
            None if environment.is_development() => Self::parse(DEFAULT_DEVELOPMENT_PUBLIC_ORIGIN)?,
            None => return Err(ConfigError::Missing(PUBLIC_ORIGIN, environment)),
        };
        if !environment.is_development() && !origin.is_https() {
            return Err(ConfigError::Invalid {
                name: PUBLIC_ORIGIN,
                reason: "must use https when APP_ENV=production".into(),
            });
        }
        Ok(origin)
    }

    pub fn as_str(&self) -> &str {
        &self.serialized
    }

    pub const fn is_https(&self) -> bool {
        self.https
    }
}

impl fmt::Display for PublicOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.serialized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_normalize_to_scheme_host_and_port() {
        for (raw, expected) in [
            ("https://app.example.com", "https://app.example.com"),
            (" https://App.Example.com/ ", "https://app.example.com"),
            ("https://app.example.com:443", "https://app.example.com"),
            (
                "https://app.example.com:8443",
                "https://app.example.com:8443",
            ),
            ("http://127.0.0.1:5173", "http://127.0.0.1:5173"),
        ] {
            assert_eq!(
                PublicOrigin::parse(raw).unwrap().as_str(),
                expected,
                "{raw}"
            );
        }
    }

    #[test]
    fn anything_beyond_an_origin_is_rejected() {
        for raw in [
            "",
            "app.example.com",
            "ftp://app.example.com",
            "https://app.example.com/app",
            "https://app.example.com/?a=1",
            "https://app.example.com/#frag",
            "https://user:pass@app.example.com",
            "file:///tmp",
        ] {
            assert!(
                PublicOrigin::parse(raw).is_err(),
                "{raw:?} should be rejected"
            );
        }
    }

    #[test]
    fn development_defaults_and_production_requires_https() {
        let none = |_: &str| None;
        let origin = PublicOrigin::from_source(Environment::Development, &none).unwrap();
        assert_eq!(origin.as_str(), DEFAULT_DEVELOPMENT_PUBLIC_ORIGIN);
        assert!(matches!(
            PublicOrigin::from_source(Environment::Production, &none),
            Err(ConfigError::Missing(PUBLIC_ORIGIN, Environment::Production))
        ));
        let http = |name: &str| (name == PUBLIC_ORIGIN).then(|| "http://app.example.com".into());
        assert!(PublicOrigin::from_source(Environment::Production, &http).is_err());
        assert!(PublicOrigin::from_source(Environment::Development, &http).is_ok());
        let https = |name: &str| (name == PUBLIC_ORIGIN).then(|| "https://app.example.com".into());
        assert!(PublicOrigin::from_source(Environment::Production, &https).is_ok());
    }
}
