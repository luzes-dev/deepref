use reqwest::header;
use std::time::Duration;

use crate::{CrossrefError, StatusClassification, classify_status, wire::CrossrefWorksResponse};

#[derive(Debug, Clone)]
pub struct CrossrefClient {
    client: reqwest::Client,
    base_url: String,
    mailto: String,
    user_agent: String,
    max_attempts: usize,
}

impl CrossrefClient {
    pub fn new(mailto: impl Into<String>) -> Result<Self, CrossrefError> {
        Self::with_base_url("https://api.crossref.org", mailto)
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        mailto: impl Into<String>,
    ) -> Result<Self, CrossrefError> {
        let mailto = mailto.into();
        if mailto.trim().is_empty() {
            return Err(CrossrefError::MissingMailto);
        }
        let user_agent = format!("deepref/0.1 (mailto:{mailto})");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(user_agent.clone())
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            mailto,
            user_agent,
            max_attempts: 5,
        })
    }

    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    pub async fn fetch_work(
        &self,
        doi: &str,
    ) -> Result<deepref_core::WorkWithReferences, CrossrefError> {
        let doi = deepref_core::normalize_doi(doi)?;
        let encoded = urlencoding::encode(&doi);
        let url = format!("{}/works/{}?mailto={}", self.base_url, encoded, self.mailto);

        let mut last_retryable = None;
        for attempt in 1..=self.max_attempts {
            let response = self
                .client
                .get(&url)
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, &self.user_agent)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let raw: serde_json::Value = response.json().await?;
                    let message_raw = raw.get("message").cloned().unwrap_or_default();
                    let body: CrossrefWorksResponse = serde_json::from_value(raw)?;
                    return Ok(body.into_domain(doi, message_raw));
                }
                Ok(response) => match classify_status(response.status()) {
                    StatusClassification::NotFound => return Err(CrossrefError::NotFound(doi)),
                    StatusClassification::Blocked => {
                        return Err(CrossrefError::Blocked(response.status()));
                    }
                    StatusClassification::Retryable => {
                        last_retryable = Some(CrossrefError::RetryableStatus(response.status()));
                        sleep_backoff(attempt).await;
                    }
                    StatusClassification::NonRetryable => {
                        return Err(CrossrefError::NonRetryableStatus(response.status()));
                    }
                },
                Err(error) if error.is_timeout() || error.is_connect() => {
                    last_retryable = Some(CrossrefError::Request(error));
                    sleep_backoff(attempt).await;
                }
                Err(error) => return Err(CrossrefError::Request(error)),
            }
        }

        Err(last_retryable.unwrap_or(CrossrefError::RetryableStatus(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        )))
    }
}

async fn sleep_backoff(attempt: usize) {
    let millis = 250_u64.saturating_mul(2_u64.saturating_pow(attempt as u32 - 1));
    tokio::time::sleep(Duration::from_millis(millis.min(5_000))).await;
}
