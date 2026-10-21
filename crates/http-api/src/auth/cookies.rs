//! Cookie names, parsing and attributes for the browser session.
//!
//! The session cookie is `HttpOnly`, `SameSite=Lax` and `Path=/`. Under https
//! it is also `Secure` and takes the `__Host-` prefix, which makes browsers
//! refuse a copy that carries a `Domain` or comes from a non-secure origin, so
//! a sibling subdomain cannot plant one.

use axum::http::{HeaderMap, HeaderValue, header};
use deepref_config::PublicOrigin;

pub(crate) const SESSION_COOKIE_MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;
pub(crate) const LOGIN_COOKIE_MAX_AGE_SECS: i64 = 10 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CookiePolicy {
    secure: bool,
}

impl CookiePolicy {
    pub(crate) fn for_origin(origin: &PublicOrigin) -> Self {
        Self {
            secure: origin.is_https(),
        }
    }

    pub(crate) const fn session_name(self) -> &'static str {
        if self.secure {
            "__Host-deepref_session"
        } else {
            "deepref_session"
        }
    }

    pub(crate) const fn login_name(self) -> &'static str {
        if self.secure {
            "__Host-deepref_login"
        } else {
            "deepref_login"
        }
    }

    pub(crate) fn set(self, name: &str, value: &str, max_age_secs: i64) -> Option<HeaderValue> {
        let secure = if self.secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; Max-Age={max_age_secs}; HttpOnly; SameSite=Lax{secure}"
        ))
        .ok()
    }

    pub(crate) fn clear(self, name: &str) -> Option<HeaderValue> {
        self.set(name, "", 0)
    }
}

/// The value of the first cookie called `name`, across all `Cookie` headers.
pub(crate) fn read_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(origin: &str) -> CookiePolicy {
        CookiePolicy::for_origin(&PublicOrigin::parse(origin).unwrap_or_else(|_| unreachable!()))
    }

    #[test]
    fn production_cookies_are_secure_and_host_prefixed() {
        let policy = policy("https://app.example.com");
        assert_eq!(policy.session_name(), "__Host-deepref_session");
        let set = policy.set(policy.session_name(), "v", 60);
        let set = set.as_ref().and_then(|v| v.to_str().ok()).unwrap_or("");
        for attribute in ["Path=/", "HttpOnly", "SameSite=Lax", "Secure"] {
            assert!(set.contains(attribute), "{attribute} in {set}");
        }
        assert!(!set.contains("Domain"));
    }

    #[test]
    fn development_cookies_are_not_secure_or_prefixed() {
        let policy = policy("http://127.0.0.1:5173");
        assert_eq!(policy.session_name(), "deepref_session");
        let set = policy.set(policy.session_name(), "v", 60);
        let set = set.as_ref().and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(set.contains("HttpOnly") && !set.contains("Secure"));
    }

    #[test]
    fn reads_the_named_cookie_among_several() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; deepref_session=tok=="),
        );
        headers.append(header::COOKIE, HeaderValue::from_static("b=2"));
        assert_eq!(read_cookie(&headers, "deepref_session"), Some("tok=="));
        assert_eq!(read_cookie(&headers, "b"), Some("2"));
        assert_eq!(read_cookie(&headers, "deepref"), None);
    }
}
