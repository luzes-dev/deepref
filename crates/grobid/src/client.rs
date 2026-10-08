use std::time::Duration;

use reqwest::{StatusCode, multipart};
use thiserror::Error;

use crate::tei::{TeiDocument, TeiError, parse_tei};

#[derive(Debug, Clone)]
pub struct GrobidConfig {
    pub base_url: String,
    pub timeout: Duration,
    /// How many times a busy (HTTP 503) server is retried.
    pub busy_retries: u32,
    pub busy_backoff: Duration,
}

impl GrobidConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            timeout: Duration::from_secs(120),
            busy_retries: 3,
            busy_backoff: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Error)]
pub enum GrobidError {
    #[error("GROBID URL is invalid: {0}")]
    InvalidUrl(String),
    #[error("GROBID request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("GROBID request timed out")]
    Timeout,
    #[error("GROBID returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("GROBID is busy (HTTP 503) after {0} retries")]
    Busy(u32),
    #[error("GROBID response could not be parsed: {0}")]
    Tei(#[from] TeiError),
}

#[derive(Debug, Clone)]
pub struct GrobidClient {
    http: reqwest::Client,
    config: GrobidConfig,
}

impl GrobidClient {
    pub fn new(config: GrobidConfig) -> Result<Self, GrobidError> {
        let trimmed = config.base_url.trim().trim_end_matches('/').to_owned();
        if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
            return Err(GrobidError::InvalidUrl(
                "expected an http(s) URL".to_owned(),
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            http,
            config: GrobidConfig {
                base_url: trimmed,
                ..config
            },
        })
    }

    pub async fn is_alive(&self) -> bool {
        match self
            .http
            .get(format!("{}/api/isalive", self.config.base_url))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Runs `processFulltextDocument` and parses the TEI response.
    pub async fn process_fulltext(&self, pdf: &[u8]) -> Result<TeiDocument, GrobidError> {
        let url = format!("{}/api/processFulltextDocument", self.config.base_url);
        let mut attempt = 0;
        loop {
            let part = multipart::Part::bytes(pdf.to_vec())
                .file_name("document.pdf")
                .mime_str("application/pdf")?;
            let form = multipart::Form::new()
                .part("input", part)
                .text("consolidateHeader", "0")
                .text("consolidateCitations", "0")
                .text("includeRawCitations", "1")
                .text("segmentSentences", "0")
                .text("teiCoordinates", "head")
                .text("teiCoordinates", "ref")
                .text("teiCoordinates", "biblStruct");
            let response = match self.http.post(&url).multipart(form).send().await {
                Ok(response) => response,
                Err(error) if error.is_timeout() => return Err(GrobidError::Timeout),
                Err(error) => return Err(error.into()),
            };
            let status = response.status();
            if status == StatusCode::SERVICE_UNAVAILABLE {
                if attempt >= self.config.busy_retries {
                    return Err(GrobidError::Busy(attempt));
                }
                attempt += 1;
                tokio::time::sleep(self.config.busy_backoff * attempt).await;
                continue;
            }
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) if error.is_timeout() => return Err(GrobidError::Timeout),
                Err(error) => return Err(error.into()),
            };
            if !status.is_success() {
                return Err(GrobidError::Status {
                    status: status.as_u16(),
                    body: body.chars().take(300).collect(),
                });
            }
            return Ok(parse_tei(&body)?);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_urls() {
        assert!(matches!(
            GrobidClient::new(GrobidConfig::new("ftp://x")),
            Err(GrobidError::InvalidUrl(_))
        ));
        assert!(GrobidClient::new(GrobidConfig::new("http://127.0.0.1:8070/")).is_ok());
    }
}
