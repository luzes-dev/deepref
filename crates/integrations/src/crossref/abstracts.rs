//! Abstract cleanup and best-effort enrichment from Europe PMC / OpenAlex.
//!
//! Crossref frequently has no abstract (and when it has one, it is JATS
//! markup). Enrichment never fails the caller: every error becomes `None`.

use deepref_application::{OutboundRequest, ProviderPermit};
use deepref_config::OpenAlexCorpus;
use reqwest::{StatusCode, header};
use serde_json::Value;
use std::time::Duration;

use crate::openalex::OpenAlexClient;

/// Requests per second the worker spends on Europe PMC across all pods.
pub const EUROPE_PMC_RATE_PER_SECOND: u32 = 5;

pub(super) const EUROPE_PMC_BASE: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest";
const OPENALEX_BASE: &str = "https://api.openalex.org";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ATTEMPTS: usize = 2;

/// Why an enricher could not be built.
pub type EnricherBuildError = Box<dyn std::error::Error + Send + Sync>;

/// Where an enriched abstract came from (stored in the record's raw payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbstractSource {
    Crossref,
    EuropePmc,
    OpenAlex,
}

impl AbstractSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crossref => "crossref",
            Self::EuropePmc => "europepmc",
            Self::OpenAlex => "openalex",
        }
    }
}

/// Whether the abstract lookup asks OpenAlex. A chain that already asked OpenAlex for the
/// DOI skips that step, since asking again repeats the same call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAlexLookup {
    Enabled,
    Disabled,
}

/// Fills a missing abstract from Europe PMC and then OpenAlex. Every request takes a permit
/// for its own service. The OpenAlex step goes through the shared OpenAlex client, so it uses
/// the metadata adapter's key state, rate and redirect handling.
#[derive(Debug, Clone)]
pub struct AbstractEnricher {
    client: reqwest::Client,
    europe_pmc_base: String,
    openalex: OpenAlexClient,
}

impl AbstractEnricher {
    /// An enricher with its own keyless OpenAlex client. Production wiring passes the shared
    /// client to `with_openalex` instead.
    pub fn new(mailto: impl Into<String>) -> Result<Self, EnricherBuildError> {
        Self::with_base_urls(EUROPE_PMC_BASE, OPENALEX_BASE, mailto)
    }

    pub fn with_base_urls(
        europe_pmc_base: impl Into<String>,
        openalex_base: impl Into<String>,
        mailto: impl Into<String>,
    ) -> Result<Self, EnricherBuildError> {
        let mailto = mailto.into();
        let openalex = OpenAlexClient::with_base_url(
            openalex_base,
            mailto.clone(),
            None,
            OpenAlexCorpus::Core,
        )?;
        Self::with_openalex(europe_pmc_base, openalex, mailto)
    }

    /// An enricher that asks OpenAlex through the process's shared client.
    pub fn with_openalex(
        europe_pmc_base: impl Into<String>,
        openalex: OpenAlexClient,
        mailto: impl Into<String>,
    ) -> Result<Self, EnricherBuildError> {
        let mailto = mailto.into();
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(format!("deepref/0.1 (mailto:{mailto})"))
            .build()?;
        Ok(Self {
            client,
            europe_pmc_base: europe_pmc_base.into().trim_end_matches('/').to_owned(),
            openalex,
        })
    }

    /// Looks the DOI up in Europe PMC, then in OpenAlex unless `openalex` disables that
    /// step. Returns cleaned plain text.
    pub async fn lookup(
        &self,
        doi: &str,
        openalex: OpenAlexLookup,
        permit: &dyn ProviderPermit,
    ) -> Option<(String, AbstractSource)> {
        let query = format!("DOI:\"{doi}\"");
        let url = format!(
            "{}/search?query={}&format=json&resultType=core&pageSize=5",
            self.europe_pmc_base,
            urlencoding::encode(&query)
        );
        if let Some(body) = self.get_json(&url, permit).await
            && let Some(text) = parse_europe_pmc(&body, doi)
        {
            return Some((text, AbstractSource::EuropePmc));
        }
        if openalex == OpenAlexLookup::Disabled {
            return None;
        }
        if let Some(body) = self.openalex.abstract_index(doi, permit).await
            && let Some(text) = parse_openalex(&body)
        {
            return Some((text, AbstractSource::OpenAlex));
        }
        None
    }

    /// A Europe PMC GET under the best-effort retry policy, with a permit before each request.
    async fn get_json(&self, url: &str, permit: &dyn ProviderPermit) -> Option<Value> {
        for attempt in 1..=MAX_ATTEMPTS {
            permit.acquire(OutboundRequest::europe_pmc()).await.ok()?;
            let response = self
                .client
                .get(url)
                .header(header::ACCEPT, "application/json")
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    return response.json().await.ok();
                }
                Ok(response)
                    if response.status() == StatusCode::TOO_MANY_REQUESTS
                        || response.status().is_server_error() =>
                {
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                Ok(_) => return None,
                Err(error) if error.is_timeout() || error.is_connect() => {
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                Err(_) => return None,
            }
        }
        None
    }
}

/// Picks the abstract of the result whose DOI matches (case-insensitive).
pub(crate) fn parse_europe_pmc(body: &Value, doi: &str) -> Option<String> {
    body.pointer("/resultList/result")?
        .as_array()?
        .iter()
        .filter(|item| {
            item.get("doi")
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(doi))
        })
        .filter_map(|item| item.get("abstractText").and_then(Value::as_str))
        .find_map(clean_abstract)
}

pub(crate) fn parse_openalex(body: &Value) -> Option<String> {
    let index = body.get("abstract_inverted_index")?.as_object()?;
    let mut words: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in index {
        for position in positions.as_array()? {
            words.push((position.as_u64()?, word.as_str()));
        }
    }
    words.sort_by_key(|(position, _)| *position);
    let joined = words
        .into_iter()
        .map(|(_, word)| word)
        .collect::<Vec<_>>()
        .join(" ");
    clean_abstract(&joined)
}

enum Segment {
    Heading(String),
    Text(String),
}

/// Converts HTML/JATS abstract markup to plain text. Paragraphs are separated
/// by a blank line; section headings become a `Label: ` prefix on the
/// following paragraph. A bare "Abstract" heading is dropped.
#[must_use]
pub fn clean_abstract(input: &str) -> Option<String> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut buffer = String::new();
    let mut in_heading = false;
    let mut rest = input;

    let flush = |buffer: &mut String, in_heading: bool, segments: &mut Vec<Segment>| {
        let collapsed = buffer.split_whitespace().collect::<Vec<_>>().join(" ");
        buffer.clear();
        if collapsed.is_empty() {
            return;
        }
        segments.push(if in_heading {
            Segment::Heading(collapsed)
        } else {
            Segment::Text(collapsed)
        });
    };

    while let Some((text, after_open)) = rest.split_once('<') {
        buffer.push_str(text);
        let Some((tag, after_tag)) = after_open.split_once('>') else {
            // Stray '<' with no closing '>': keep it as text.
            buffer.push('<');
            buffer.push_str(after_open);
            rest = "";
            break;
        };
        rest = after_tag;
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let name = name.rsplit(':').next().unwrap_or("");
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "title" | "label" => {
                flush(&mut buffer, in_heading, &mut segments);
                in_heading = !closing;
            }
            "p" | "br" | "sec" | "div" | "li" | "ul" | "ol" | "list-item" | "list" => {
                flush(&mut buffer, in_heading, &mut segments);
            }
            _ => {}
        }
    }
    buffer.push_str(rest);
    flush(&mut buffer, in_heading, &mut segments);

    let mut paragraphs: Vec<String> = Vec::new();
    let mut label: Option<String> = None;
    for segment in segments {
        match segment {
            Segment::Heading(text) => {
                let text = decode_entities(&text);
                let text = text.trim_end_matches(':').trim().to_owned();
                if let Some(previous) = label.replace(text) {
                    paragraphs.push(previous);
                }
            }
            Segment::Text(text) => {
                let text = decode_entities(&text);
                paragraphs.push(match label.take() {
                    Some(label) => format!("{label}: {text}"),
                    None => text,
                });
            }
        }
    }
    if let Some(label) = label {
        paragraphs.push(label);
    }
    paragraphs.retain(|p| !p.eq_ignore_ascii_case("abstract") && !p.trim().is_empty());
    // A leading "Abstract" label was swallowed into the first paragraph.
    if let Some(first) = paragraphs.first_mut()
        && let Some(stripped) = first.strip_prefix("Abstract: ")
    {
        *first = stripped.to_owned();
    }
    let joined = paragraphs.join("\n\n");
    (!joined.is_empty()).then_some(joined)
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some((before, tail)) = rest.split_once('&') {
        out.push_str(before);
        let decoded = tail
            .split_once(';')
            .filter(|(entity, _)| entity.len() <= 10)
            .and_then(|(entity, after)| {
                let value = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    _ => entity.strip_prefix('#').and_then(|number| {
                        let code = match number.strip_prefix(['x', 'X']) {
                            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                            None => number.parse().ok()?,
                        };
                        char::from_u32(code)
                    }),
                };
                value.map(|c| (c, after))
            });
        match decoded {
            Some((c, after)) => {
                out.push(c);
                rest = after;
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_permit::RecordingPermit;
    use serde_json::json;

    #[test]
    fn europe_pmc_headings_become_labels() {
        let text = clean_abstract(
            "<h4>Background</h4>Fitbit &amp; web use.<h4>Methods</h4>n=51 women (BMI &#8805;25.0), <i>randomized</i>.",
        )
        .unwrap();
        assert_eq!(
            text,
            "Background: Fitbit & web use.\n\nMethods: n=51 women (BMI ≥25.0), randomized."
        );
    }

    #[test]
    fn crossref_jats_is_flattened_and_abstract_title_dropped() {
        let text = clean_abstract(
            "<jats:title>Abstract</jats:title><jats:p>First <jats:italic>para</jats:italic>.</jats:p>\n<jats:p>Second.</jats:p>",
        )
        .unwrap();
        assert_eq!(text, "First para.\n\nSecond.");
    }

    #[test]
    fn empty_or_markup_only_input_is_none() {
        assert_eq!(clean_abstract("  <p> </p> "), None);
        assert_eq!(clean_abstract("<jats:title>Abstract</jats:title>"), None);
    }

    #[test]
    fn stray_angle_bracket_is_kept() {
        assert_eq!(
            clean_abstract("p < 0.05 overall").unwrap(),
            "p < 0.05 overall"
        );
    }

    #[test]
    fn inverted_index_is_reconstructed_in_position_order() {
        let body = json!({"abstract_inverted_index": {"world": [1, 3], "hello": [0], "big": [2]}});
        assert_eq!(parse_openalex(&body).unwrap(), "hello world big world");
        assert_eq!(
            parse_openalex(&json!({"abstract_inverted_index": null})),
            None
        );
    }

    #[test]
    fn europe_pmc_requires_matching_doi() {
        let body = json!({"resultList": {"result": [
            {"doi": "10.1/other", "abstractText": "Wrong"},
            {"doi": "10.1/ABC", "abstractText": "<h4>Aim</h4>Right"}
        ]}});
        assert_eq!(parse_europe_pmc(&body, "10.1/abc").unwrap(), "Aim: Right");
        assert_eq!(parse_europe_pmc(&body, "10.1/none"), None);
    }

    /// Serves one `200` reply per entry, in order, one connection each, then stops.
    async fn serve(replies: Vec<&'static str>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            for body in replies {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut request = [0_u8; 4096];
                let _ = socket.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{address}")
    }

    const NO_EUROPE_PMC_MATCH: &str = r#"{"resultList":{"result":[]}}"#;
    const OPENALEX_ABSTRACT: &str = r#"{"abstract_inverted_index":{"Hello":[0],"world":[1]}}"#;

    #[tokio::test]
    async fn openalex_is_asked_only_when_the_lookup_enables_it() {
        let europe_pmc = serve(vec![NO_EUROPE_PMC_MATCH, NO_EUROPE_PMC_MATCH]).await;
        let openalex = serve(vec![OPENALEX_ABSTRACT]).await;
        let enricher = AbstractEnricher::with_base_urls(europe_pmc, openalex, "a@b.co")
            .expect("enricher should build");

        assert_eq!(
            enricher
                .lookup(
                    "10.1/x",
                    OpenAlexLookup::Disabled,
                    &RecordingPermit::default()
                )
                .await,
            None,
            "the chain already asked OpenAlex, so the abstract step skips it"
        );
        assert_eq!(
            enricher
                .lookup(
                    "10.1/x",
                    OpenAlexLookup::Enabled,
                    &RecordingPermit::default()
                )
                .await,
            Some(("Hello world".to_owned(), AbstractSource::OpenAlex))
        );
    }

    #[tokio::test]
    async fn europe_pmc_still_answers_when_openalex_is_disabled() {
        const EUROPE_PMC_ABSTRACT: &str =
            r#"{"resultList":{"result":[{"doi":"10.1/x","abstractText":"From Europe PMC"}]}}"#;
        let europe_pmc = serve(vec![EUROPE_PMC_ABSTRACT]).await;
        let openalex = serve(vec![OPENALEX_ABSTRACT]).await;
        let enricher = AbstractEnricher::with_base_urls(europe_pmc, openalex, "a@b.co")
            .expect("enricher should build");

        assert_eq!(
            enricher
                .lookup(
                    "10.1/x",
                    OpenAlexLookup::Disabled,
                    &RecordingPermit::default()
                )
                .await,
            Some(("From Europe PMC".to_owned(), AbstractSource::EuropePmc))
        );
    }

    #[tokio::test]
    async fn unreachable_services_yield_none_without_error() {
        let enricher =
            AbstractEnricher::with_base_urls("http://127.0.0.1:1", "http://127.0.0.1:1", "a@b.co")
                .unwrap();
        assert_eq!(
            enricher
                .lookup(
                    "10.1/x",
                    OpenAlexLookup::Enabled,
                    &RecordingPermit::default()
                )
                .await,
            None
        );
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn live_europe_pmc_lookup() {
        let enricher = AbstractEnricher::new("test@example.com").unwrap();
        let (text, source) = enricher
            .lookup(
                "10.1016/j.amepre.2015.01.020",
                OpenAlexLookup::Enabled,
                &RecordingPermit::default(),
            )
            .await
            .unwrap();
        assert_eq!(source, AbstractSource::EuropePmc);
        assert!(text.starts_with("Introduction: "));
    }
}
