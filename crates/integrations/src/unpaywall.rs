//! Unpaywall: the location of an open-access PDF for a DOI.
//!
//! Unpaywall asks every caller to identify itself with a contact address, so the caller
//! passes the workspace's configured contact e-mail, never a personal one.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;
use thiserror::Error;

const UNPAYWALL_BASE: &str = "https://api.unpaywall.org";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum UnpaywallError {
    #[error("Unpaywall needs a contact e-mail address")]
    MissingEmail,
    #[error("Unpaywall rejected the request ({0})")]
    Rejected(StatusCode),
    #[error("Unpaywall is busy right now ({0})")]
    Unavailable(StatusCode),
    #[error("Unpaywall request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Unpaywall answer is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid DOI: {0}")]
    InvalidDoi(#[from] deepref_domain::DoiError),
}

#[derive(Debug, Clone)]
pub struct UnpaywallClient {
    client: reqwest::Client,
    base_url: String,
    email: String,
}

impl UnpaywallClient {
    pub fn new(email: impl Into<String>) -> Result<Self, UnpaywallError> {
        Self::with_base_url(UNPAYWALL_BASE, email)
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        email: impl Into<String>,
    ) -> Result<Self, UnpaywallError> {
        let email = email.into().trim().to_owned();
        if email.is_empty() {
            return Err(UnpaywallError::MissingEmail);
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(format!("deepref/0.1 (mailto:{email})"))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            email,
        })
    }

    /// The PDF URL of the best open-access copy of `doi`. `Ok(None)` when Unpaywall does not know
    /// the DOI, or knows only a landing page and no PDF file.
    pub async fn open_access_pdf_url(&self, doi: &str) -> Result<Option<String>, UnpaywallError> {
        let doi = deepref_domain::normalize_doi(doi)?;
        let url = format!(
            "{}/v2/{}?email={}",
            self.base_url,
            urlencoding::encode(&doi),
            urlencoding::encode(&self.email)
        );
        let response = self
            .client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::TOO_MANY_REQUESTS => Err(UnpaywallError::Unavailable(status)),
            _ if status.is_server_error() => Err(UnpaywallError::Unavailable(status)),
            _ if status.is_success() => {
                let body: Value = response.json().await?;
                Ok(pdf_url_from_response(&body))
            }
            _ => Err(UnpaywallError::Rejected(status)),
        }
    }
}

/// The PDF location Unpaywall reports: the best copy first, then any other copy that has a PDF
/// file. Only https locations are used, because external documents must be fetched over https.
pub fn pdf_url_from_response(body: &Value) -> Option<String> {
    let best = body
        .pointer("/best_oa_location/url_for_pdf")
        .and_then(Value::as_str);
    let others = body
        .get("oa_locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| location.get("url_for_pdf").and_then(Value::as_str));
    best.into_iter()
        .chain(others)
        .find(|url| url.starts_with("https://"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_the_best_location_then_any_pdf_location() {
        let body = json!({
            "best_oa_location": { "url_for_pdf": "https://repo.example.org/best.pdf" },
            "oa_locations": [
                { "url_for_pdf": "https://other.example.org/copy.pdf" }
            ]
        });
        assert_eq!(
            pdf_url_from_response(&body).as_deref(),
            Some("https://repo.example.org/best.pdf")
        );
    }

    #[test]
    fn falls_back_when_the_best_location_has_no_pdf_file() {
        let body = json!({
            "best_oa_location": { "url_for_pdf": null, "url": "https://landing.example.org/a" },
            "oa_locations": [
                { "url_for_pdf": null },
                { "url_for_pdf": "https://other.example.org/copy.pdf" }
            ]
        });
        assert_eq!(
            pdf_url_from_response(&body).as_deref(),
            Some("https://other.example.org/copy.pdf")
        );
    }

    #[test]
    fn ignores_insecure_and_landing_page_only_answers() {
        let insecure = json!({
            "best_oa_location": { "url_for_pdf": "http://plain.example.org/a.pdf" }
        });
        assert_eq!(pdf_url_from_response(&insecure), None);
        let landing_only = json!({
            "best_oa_location": { "url": "https://landing.example.org/a" },
            "oa_locations": []
        });
        assert_eq!(pdf_url_from_response(&landing_only), None);
    }

    #[test]
    fn an_empty_contact_address_is_refused_before_any_request() {
        assert!(matches!(
            UnpaywallClient::new("   "),
            Err(UnpaywallError::MissingEmail)
        ));
    }

    /// Serves one HTTP response with the given status line and JSON body, then stops.
    async fn one_shot_server(status_line: &'static str, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0_u8; 2048];
                let _ = socket.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn unknown_doi_is_not_an_error() {
        let base = one_shot_server("404 Not Found", "{}").await;
        let client = UnpaywallClient::with_base_url(base, "contact@example.org").unwrap();
        assert_eq!(
            client.open_access_pdf_url("10.1000/unknown").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn open_access_pdf_is_read_from_the_answer() {
        let body = r#"{"best_oa_location":{"url_for_pdf":"https://repo.example.org/x.pdf"}}"#;
        let base = one_shot_server("200 OK", body).await;
        let client = UnpaywallClient::with_base_url(base, "contact@example.org").unwrap();
        assert_eq!(
            client
                .open_access_pdf_url("https://doi.org/10.1000/Example")
                .await
                .unwrap()
                .as_deref(),
            Some("https://repo.example.org/x.pdf")
        );
    }

    #[tokio::test]
    async fn an_invalid_contact_address_is_reported_as_rejected() {
        let base = one_shot_server("422 Unprocessable Entity", "{}").await;
        let client = UnpaywallClient::with_base_url(base, "contact@example.org").unwrap();
        assert!(matches!(
            client.open_access_pdf_url("10.1000/x").await,
            Err(UnpaywallError::Rejected(_))
        ));
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn live_unpaywall_lookup_for_an_open_access_doi() {
        let client = UnpaywallClient::new("contact@example.org").unwrap();
        let url = client
            .open_access_pdf_url("10.1371/journal.pone.0000308")
            .await
            .unwrap();
        assert!(url.is_some_and(|url| url.starts_with("https://")));
    }
}
