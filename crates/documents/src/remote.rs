use std::{future::Future, net::IpAddr, pin::Pin, time::Duration};

use bytes::Bytes;
use futures::{StreamExt, stream};
use reqwest::{StatusCode, header, redirect::Policy};
use thiserror::Error;

use crate::{DocumentStore, DocumentStoreError, StoredObject};

#[derive(Debug, Clone, Copy)]
pub struct FetchPolicy {
    pub timeout: Duration,
    pub max_redirects: usize,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_redirects: 3,
        }
    }
}

#[derive(Debug, Error)]
pub enum RemoteFetchError {
    #[error("external document URL must be HTTPS without credentials or a fragment")]
    InvalidUrl,
    #[error("external document host resolves to a private or reserved address")]
    ForbiddenAddress,
    #[error("external document host could not be resolved")]
    Resolution,
    #[error("external document exceeded the redirect limit")]
    RedirectLimit,
    #[error("external document redirect is invalid")]
    InvalidRedirect,
    #[error("external document returned HTTP {0}")]
    Http(StatusCode),
    #[error("external document has an unsupported content type")]
    InvalidContentType,
    #[error("external document is too large")]
    TooLarge,
    #[error("external document does not have a PDF signature")]
    InvalidSignature,
    #[error("external document request failed: {0}")]
    Request(String),
    #[error(transparent)]
    Store(#[from] DocumentStoreError),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HttpsPdfFetcher {
    policy: FetchPolicy,
}

pub type RemoteFetchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(StoredObject, url::Url), RemoteFetchError>> + Send + 'a>>;

pub trait RemoteDocumentFetcher: Send + Sync {
    fn fetch<'a>(&'a self, input: &'a str, store: &'a DocumentStore) -> RemoteFetchFuture<'a>;
}

impl RemoteDocumentFetcher for HttpsPdfFetcher {
    fn fetch<'a>(&'a self, input: &'a str, store: &'a DocumentStore) -> RemoteFetchFuture<'a> {
        Box::pin(self.fetch_into_store(input, store))
    }
}

impl HttpsPdfFetcher {
    pub const fn new(policy: FetchPolicy) -> Self {
        Self { policy }
    }

    pub async fn fetch_into_store(
        &self,
        input: &str,
        store: &DocumentStore,
    ) -> Result<(StoredObject, url::Url), RemoteFetchError> {
        let mut url = validate_external_url(input)?;
        for redirect_count in 0..=self.policy.max_redirects {
            let (host, addresses) = resolve_public_addresses(&url).await?;
            let client = reqwest::Client::builder()
                .redirect(Policy::none())
                .timeout(self.policy.timeout)
                .no_proxy()
                .resolve_to_addrs(&host, &addresses)
                .build()
                .map_err(|error| RemoteFetchError::Request(error.to_string()))?;
            let response = client
                .get(url.clone())
                .send()
                .await
                .map_err(|error| RemoteFetchError::Request(error.to_string()))?;
            if response
                .remote_addr()
                .is_some_and(|address| is_forbidden_ip(address.ip()))
            {
                return Err(RemoteFetchError::ForbiddenAddress);
            }
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(RemoteFetchError::InvalidRedirect)?;
                url = next_redirect(&url, location, redirect_count, self.policy.max_redirects)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(RemoteFetchError::Http(response.status()));
            }
            validate_content_type(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
            )?;
            validate_content_length(response.content_length(), store.max_bytes())?;
            let mut body = response.bytes_stream();
            let mut prefix = Vec::with_capacity(5);
            let mut buffered = Vec::new();
            while prefix.len() < 5 {
                let chunk = body
                    .next()
                    .await
                    .ok_or(RemoteFetchError::InvalidSignature)?
                    .map_err(|error| RemoteFetchError::Request(error.to_string()))?;
                let needed = 5 - prefix.len();
                prefix.extend_from_slice(&chunk[..chunk.len().min(needed)]);
                buffered.push(chunk);
            }
            validate_pdf_signature(&prefix)?;
            let initial = stream::iter(buffered.into_iter().map(Ok::<Bytes, reqwest::Error>));
            let stored = store.put_stream(initial.chain(body)).await?;
            return Ok((stored, url));
        }
        Err(RemoteFetchError::RedirectLimit)
    }
}

fn next_redirect(
    current: &url::Url,
    location: &str,
    redirect_count: usize,
    maximum: usize,
) -> Result<url::Url, RemoteFetchError> {
    if redirect_count == maximum {
        return Err(RemoteFetchError::RedirectLimit);
    }
    let redirected = current
        .join(location)
        .map_err(|_| RemoteFetchError::InvalidRedirect)?;
    validate_external_url(redirected.as_str())
}

fn validate_content_length(length: Option<u64>, maximum: usize) -> Result<(), RemoteFetchError> {
    if length.is_some_and(|value| value > maximum as u64) {
        Err(RemoteFetchError::TooLarge)
    } else {
        Ok(())
    }
}

fn validate_pdf_signature(prefix: &[u8]) -> Result<(), RemoteFetchError> {
    if prefix == b"%PDF-" {
        Ok(())
    } else {
        Err(RemoteFetchError::InvalidSignature)
    }
}

pub fn validate_external_url(value: &str) -> Result<url::Url, RemoteFetchError> {
    let url = url::Url::parse(value).map_err(|_| RemoteFetchError::InvalidUrl)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(RemoteFetchError::InvalidUrl);
    }
    if url
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(is_forbidden_ip)
    {
        return Err(RemoteFetchError::ForbiddenAddress);
    }
    Ok(url)
}

fn validate_content_type(value: Option<&str>) -> Result<(), RemoteFetchError> {
    let mime = value
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default();
    if matches!(mime, "application/pdf" | "application/octet-stream") {
        Ok(())
    } else {
        Err(RemoteFetchError::InvalidContentType)
    }
}

async fn resolve_public_addresses(
    url: &url::Url,
) -> Result<(String, Vec<std::net::SocketAddr>), RemoteFetchError> {
    let host = url
        .host_str()
        .ok_or(RemoteFetchError::InvalidUrl)?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or(RemoteFetchError::InvalidUrl)?;
    let addresses = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| RemoteFetchError::Resolution)?
        .collect::<Vec<_>>();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| is_forbidden_ip(address.ip()))
    {
        return Err(RemoteFetchError::ForbiddenAddress);
    }
    Ok((host, addresses))
}

fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                || octets[0] >= 240
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || segments[0] & 0xfe00 == 0xfc00
                || segments[0] & 0xffc0 == 0xfe80
                || (segments[0] == 0x2001 && matches!(segments[1], 0x0000 | 0x0002 | 0x0db8))
                || (segments[0] == 0x2001 && segments[1] & 0xfff0 == 0x0010)
                || segments[0] == 0x3fff
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| is_forbidden_ip(IpAddr::V4(mapped)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_unsafe_urls_and_addresses() {
        assert!(validate_external_url("http://example.com/a.pdf").is_err());
        assert!(validate_external_url("https://u:p@example.com/a.pdf").is_err());
        assert!(validate_external_url("https://example.com/a.pdf#token").is_err());
        for value in ["127.0.0.1", "10.0.0.1", "169.254.169.254", "::1", "fc00::1"] {
            assert!(is_forbidden_ip(
                value.parse().expect("IP fixture must parse")
            ));
        }
    }

    #[test]
    fn validates_remote_content_type() {
        assert!(validate_content_type(Some("application/pdf; charset=binary")).is_ok());
        assert!(validate_content_type(Some("application/octet-stream")).is_ok());
        assert!(validate_content_type(Some("text/html")).is_err());
    }

    #[test]
    fn bounds_redirects_declared_sizes_and_signatures() {
        let current = url::Url::parse("https://example.com/start.pdf").unwrap();
        assert!(matches!(
            next_redirect(&current, "https://example.org/next.pdf", 3, 3),
            Err(RemoteFetchError::RedirectLimit)
        ));
        assert!(next_redirect(&current, "http://example.org/next.pdf", 0, 3).is_err());
        assert!(validate_content_length(Some(11), 10).is_err());
        assert!(validate_content_length(Some(10), 10).is_ok());
        assert!(validate_pdf_signature(b"%PDF-").is_ok());
        assert!(validate_pdf_signature(b"<html").is_err());
    }

    #[test]
    fn rejects_reserved_ipv4_and_ipv6_families() {
        for value in [
            "0.0.0.0",
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "203.0.113.1",
            "2001:db8::1",
            "2001:2::1",
            "3fff::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                is_forbidden_ip(value.parse().unwrap()),
                "{value} must be blocked"
            );
        }
        assert!(!is_forbidden_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(!is_forbidden_ip("1.1.1.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn redirect_targets_are_resolved_and_rejected_on_every_hop() {
        let current = url::Url::parse("https://example.com/start.pdf").unwrap();
        assert!(matches!(
            next_redirect(&current, "https://127.0.0.1/private.pdf", 0, 3),
            Err(RemoteFetchError::ForbiddenAddress)
        ));
        let public = next_redirect(&current, "https://1.1.1.1/public.pdf", 0, 3).unwrap();
        assert!(resolve_public_addresses(&public).await.is_ok());
    }
}
