use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum CrossrefSearchError {
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    Permanent(String),
}

impl CrossrefSearchError {
    fn retryable(message: impl Into<String>) -> Self {
        Self::Retryable(message.into())
    }

    fn permanent(message: impl Into<String>) -> Self {
        Self::Permanent(message.into())
    }
}

async fn search_response(
    client: &reqwest::Client,
    url: &str,
) -> Result<String, CrossrefSearchError> {
    let response =
        client.get(url).send().await.map_err(|_| {
            CrossrefSearchError::retryable("The literature service did not answer.")
        })?;
    let status = response.status();
    if status.is_server_error() || status.as_u16() == 429 {
        return Err(CrossrefSearchError::retryable(
            "The literature service is busy right now.",
        ));
    }
    if !status.is_success() {
        return Err(CrossrefSearchError::permanent(format!(
            "The literature service refused the request ({}).",
            status.as_u16()
        )));
    }
    response.text().await.map_err(|_| {
        CrossrefSearchError::retryable("The literature service answer could not be read.")
    })
}
fn encode(text: &str) -> String {
    reqwest::Url::parse_with_params("https://x.invalid/", [("q", text)])
        .map(|url| {
            url.query()
                .unwrap_or("q=")
                .trim_start_matches("q=")
                .to_owned()
        })
        .unwrap_or_default()
}
/// Search Crossref for works indexed since `from_date` (`YYYY-MM-DD`).
pub async fn search_crossref(
    client: &reqwest::Client,
    terms: &str,
    from_date: &str,
    mailto: &str,
) -> Result<Vec<Value>, CrossrefSearchError> {
    let mut url = format!(
        "https://api.crossref.org/works?query.bibliographic={}&filter=from-index-date:{}&rows=50&sort=indexed&order=desc&select=DOI,title,container-title,issued,author,abstract",
        encode(terms),
        encode(from_date)
    );
    if !mailto.trim().is_empty() {
        url.push_str(&format!("&mailto={}", encode(mailto.trim())));
    }
    let body = search_response(client, &url).await?;
    let parsed: Value = serde_json::from_str(&body).map_err(|_| {
        CrossrefSearchError::retryable("Crossref sent an answer that could not be understood.")
    })?;
    let items = parsed
        .pointer("/message/items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .map(|work| {
            let first = |key: &str| {
                work.get(key)
                    .and_then(Value::as_array)
                    .and_then(|list| list.first())
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let authors: Vec<String> = work
                .get("author")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|author| {
                            format!(
                                "{} {}",
                                author
                                    .get("given")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                                author
                                    .get("family")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                            )
                            .trim()
                            .to_owned()
                        })
                        .filter(|name| !name.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "source": "crossref",
                "doi": work.get("DOI").and_then(Value::as_str).map(str::to_lowercase),
                "pmid": Value::Null,
                "title": first("title"),
                "abstract": work.get("abstract").and_then(Value::as_str),
                "journal": first("container-title"),
                "year": work.pointer("/issued/date-parts/0/0").and_then(Value::as_i64),
                "authors": authors,
            })
        })
        .collect())
}
