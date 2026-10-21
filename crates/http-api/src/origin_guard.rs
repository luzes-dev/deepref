//! CSRF defence for cookie-authenticated requests.
//!
//! `SameSite=Lax` is defence in depth: it does not stop a sibling subdomain
//! (same site, different origin). The control is this check on every unsafe
//! request that presents the session cookie:
//!
//! ```text
//! Origin present  -> must equal the canonical application origin exactly
//!                    (scheme, host and port); `Origin: null` is rejected
//! Origin absent   -> Sec-Fetch-Site must be same-origin
//! both absent     -> rejected: a browser always sends one of them
//! ```
//!
//! The canonical origin comes from runtime configuration, never from the
//! `Host` header. Requests without the session cookie are not subject to the
//! rule: they authenticate some other way, or fail as unauthenticated. The
//! layer runs before any handler or body extractor, so a rejected request
//! changes no state.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    auth::{CookiePolicy, read_cookie},
    error::{ApiError, AuthFailure},
};

#[derive(Debug, Clone)]
pub(crate) struct OriginGuard(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    origin: String,
    session_cookie: &'static str,
}

impl OriginGuard {
    pub(crate) fn new(origin: &deepref_config::PublicOrigin) -> Self {
        Self(Arc::new(Inner {
            origin: origin.as_str().to_owned(),
            session_cookie: CookiePolicy::for_origin(origin).session_name(),
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allowed,
    Rejected(&'static str),
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn judge(guard: &Inner, method: &Method, headers: &HeaderMap) -> Verdict {
    if is_safe(method) || read_cookie(headers, guard.session_cookie).is_none() {
        return Verdict::Allowed;
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    match (origins.next(), origins.next()) {
        (Some(_), Some(_)) => Verdict::Rejected("multiple origin headers"),
        (Some(origin), None) if origin.as_bytes() == guard.origin.as_bytes() => Verdict::Allowed,
        (Some(_), None) => Verdict::Rejected("origin differs from the application origin"),
        (None, _) => match headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
        {
            Some("same-origin") => Verdict::Allowed,
            Some(_) => Verdict::Rejected("sec-fetch-site is not same-origin"),
            None => Verdict::Rejected("neither origin nor sec-fetch-site present"),
        },
    }
}

/// Middleware: 403 for a cookie-authenticated unsafe request from another origin.
pub(crate) async fn reject_cross_origin_session_requests(
    State(guard): State<OriginGuard>,
    request: Request,
    next: Next,
) -> Response {
    match judge(&guard.0, request.method(), request.headers()) {
        Verdict::Allowed => next.run(request).await,
        Verdict::Rejected(reason) => {
            tracing::warn!(
                reason,
                method = %request.method(),
                path = request.uri().path(),
                "rejected a cookie-authenticated unsafe request"
            );
            ApiError::Auth(AuthFailure::OriginRejected).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const ORIGIN: &str = "https://app.example.com";

    fn guard() -> Inner {
        Inner {
            origin: ORIGIN.to_owned(),
            session_cookie: "__Host-deepref_session",
        }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    const COOKIE: (&str, &str) = ("cookie", "__Host-deepref_session=sealed");

    fn verdict(method: Method, pairs: &[(&'static str, &'static str)]) -> Verdict {
        judge(&guard(), &method, &headers(pairs))
    }

    #[test]
    fn same_origin_post_is_allowed() {
        assert_eq!(
            verdict(Method::POST, &[COOKIE, ("origin", ORIGIN)]),
            Verdict::Allowed
        );
    }

    #[test]
    fn any_origin_that_is_not_exactly_the_canonical_one_is_rejected() {
        for origin in [
            "https://evil.example",
            "null",
            "http://app.example.com",
            "https://app.example.com:8443",
            "https://sibling.example.com",
            "https://app.example.com.evil.example",
            "https://APP.example.com",
            "https://app.example.com/",
        ] {
            assert!(
                matches!(
                    verdict(Method::POST, &[COOKIE, ("origin", origin)]),
                    Verdict::Rejected(_)
                ),
                "{origin}"
            );
        }
    }

    #[test]
    fn missing_origin_requires_sec_fetch_site_same_origin() {
        assert_eq!(
            verdict(Method::DELETE, &[COOKIE, ("sec-fetch-site", "same-origin")]),
            Verdict::Allowed
        );
        for site in ["same-site", "cross-site", "none"] {
            assert!(
                matches!(
                    verdict(Method::DELETE, &[COOKIE, ("sec-fetch-site", site)]),
                    Verdict::Rejected(_)
                ),
                "{site}"
            );
        }
        assert!(matches!(
            verdict(Method::PUT, &[COOKIE]),
            Verdict::Rejected(_)
        ));
    }

    #[test]
    fn origin_wins_over_sec_fetch_site() {
        assert!(matches!(
            verdict(
                Method::POST,
                &[
                    COOKIE,
                    ("origin", "https://evil.example"),
                    ("sec-fetch-site", "same-origin")
                ]
            ),
            Verdict::Rejected(_)
        ));
    }

    #[test]
    fn repeated_origin_headers_are_rejected() {
        assert!(matches!(
            verdict(
                Method::POST,
                &[COOKIE, ("origin", ORIGIN), ("origin", ORIGIN)]
            ),
            Verdict::Rejected(_)
        ));
    }

    #[test]
    fn safe_methods_and_cookieless_requests_are_not_checked() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert_eq!(
                verdict(method, &[COOKIE, ("origin", "https://evil.example")]),
                Verdict::Allowed
            );
        }
        assert_eq!(
            verdict(Method::POST, &[("origin", "https://evil.example")]),
            Verdict::Allowed
        );
        assert_eq!(verdict(Method::PATCH, &[]), Verdict::Allowed);
    }

    #[test]
    fn other_cookies_do_not_trigger_the_check() {
        assert_eq!(
            verdict(Method::POST, &[("cookie", "theme=dark")]),
            Verdict::Allowed
        );
    }
}
