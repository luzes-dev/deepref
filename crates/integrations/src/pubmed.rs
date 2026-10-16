//! PubMed E-utilities: finding PMIDs and reading article records.
//!
//! Publication alerts, the `import_identifiers` workflow block and the PMID import
//! share this client. It does not pace its own requests, because the pacing has to be
//! shared by every worker pod: callers reserve a permit before each call, at
//! [`REQUESTS_PER_SECOND`].

use deepref_application::{RawAuthor, RawIdentifier, RawRecord};
use deepref_domain::IdentifierScheme;
use quick_xml::{Reader, events::Event};
use serde_json::{Value, json};

const EUTILS: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";
/// Name of this client, sent as NCBI's `tool` parameter.
const TOOL: &str = "DeepRef";
/// The most PMIDs one `efetch` call carries.
pub const MAX_IDS_PER_FETCH: usize = 200;
/// NCBI's limit for clients without an API key.
pub const REQUESTS_PER_SECOND: u32 = 3;

const UNREADABLE: &str = "PubMed sent an answer that could not be understood.";

/// Why a PubMed request failed. The messages are worded for the people who read them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PubmedError {
    /// Worth trying again later: the service is busy, did not answer, or sent an answer
    /// that could not be read.
    Retryable(String),
    /// Will not succeed as sent.
    Permanent(String),
}

impl PubmedError {
    pub fn message(&self) -> &str {
        match self {
            Self::Retryable(message) | Self::Permanent(message) => message,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

impl std::fmt::Display for PubmedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for PubmedError {}

/// One article from an `efetch` answer. A text field is `None` when PubMed left it empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PubmedArticle {
    pub pmid: Option<String>,
    /// Lower case, as DeepRef stores DOIs.
    pub doi: Option<String>,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub journal: Option<String>,
    pub year: Option<i32>,
    /// "Forename Surname", in PubMed's order.
    pub authors: Vec<String>,
}

impl PubmedArticle {
    /// The JSON item that publication alerts keep, and that an import stores as the raw record.
    pub fn to_item(&self) -> Value {
        json!({
            "source": "pubmed",
            "pmid": self.pmid,
            "doi": self.doi,
            "title": self.title,
            "abstract": self.abstract_text,
            "journal": self.journal,
            "year": self.year,
            "authors": self.authors,
        })
    }

    /// The record an import saves for this article.
    pub fn to_raw_record(&self) -> RawRecord {
        let mut source_identifiers = Vec::new();
        if let Some(doi) = &self.doi {
            source_identifiers.push(RawIdentifier::new(IdentifierScheme::Doi, doi.clone()));
        }
        if let Some(pmid) = &self.pmid {
            source_identifiers.push(RawIdentifier::new(IdentifierScheme::Pmid, pmid.clone()));
        }
        RawRecord {
            source_identifiers,
            title: self.title.clone(),
            abstract_text: self.abstract_text.clone(),
            authors: self
                .authors
                .iter()
                .map(|author| RawAuthor::literal(author.as_str()))
                .collect(),
            publication_year: self.year,
            journal: self.journal.clone(),
            raw: self.to_item(),
        }
    }
}

/// Calls the E-utilities for one workspace. `http` should carry the server's timeout and
/// user agent.
#[derive(Debug, Clone)]
pub struct PubmedClient {
    http: reqwest::Client,
    mailto: String,
}

impl PubmedClient {
    /// `mailto` is the workspace contact address (the `crossref_mailto` setting). NCBI asks
    /// for it as `email`, so it is sent when it is set. It is never a user's own address.
    pub fn new(http: reqwest::Client, mailto: impl Into<String>) -> Self {
        Self {
            http,
            mailto: mailto.into(),
        }
    }

    /// PMIDs of records indexed in the last `days` days that match `terms`.
    pub async fn search(&self, terms: &str, days: u32) -> Result<Vec<String>, PubmedError> {
        let url = self.url(
            "esearch.fcgi",
            &format!(
                "db=pubmed&retmode=json&retmax=100&datetype=edat&reldate={days}&term={}",
                encode(terms)
            ),
        );
        let body = self.get(&url).await?;
        let parsed: Value = serde_json::from_str(&body)
            .map_err(|_| PubmedError::Retryable(UNREADABLE.to_owned()))?;
        Ok(parsed
            .pointer("/esearchresult/idlist")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Article records for `pmids`. Only the first [`MAX_IDS_PER_FETCH`] are sent. A PMID
    /// that PubMed does not know is absent from the result, so callers see it as not found.
    pub async fn fetch(&self, pmids: &[String]) -> Result<Vec<PubmedArticle>, PubmedError> {
        if pmids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = pmids
            .iter()
            .take(MAX_IDS_PER_FETCH)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let url = self.url(
            "efetch.fcgi",
            &format!("db=pubmed&retmode=xml&id={}", encode(&ids)),
        );
        let xml = self.get(&url).await?;
        parse_efetch(&xml)
    }

    fn url(&self, endpoint: &str, query: &str) -> String {
        let mut url = format!("{EUTILS}/{endpoint}?{query}&tool={TOOL}");
        let mailto = self.mailto.trim();
        if !mailto.is_empty() {
            url.push_str("&email=");
            url.push_str(&encode(mailto));
        }
        url
    }

    async fn get(&self, url: &str) -> Result<String, PubmedError> {
        let response = self.http.get(url).send().await.map_err(|_| {
            PubmedError::Retryable("The literature service did not answer.".to_owned())
        })?;
        let status = response.status();
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(PubmedError::Retryable(
                "The literature service is busy right now.".to_owned(),
            ));
        }
        if !status.is_success() {
            return Err(PubmedError::Permanent(format!(
                "The literature service refused the request ({}).",
                status.as_u16()
            )));
        }
        response.text().await.map_err(|_| {
            PubmedError::Retryable("The literature service answer could not be read.".to_owned())
        })
    }
}

/// Reads one PubMed ID from what a person types: a bare number, a `PMID:` label, or a
/// `pubmed.ncbi.nlm.nih.gov` link. Returns the digits without leading zeros, or `None`.
pub fn normalize_pmid(input: &str) -> Option<String> {
    let mut text = input.trim();
    for prefix in [
        "https://pubmed.ncbi.nlm.nih.gov/",
        "http://pubmed.ncbi.nlm.nih.gov/",
        "pubmed.ncbi.nlm.nih.gov/",
    ] {
        if let Some(rest) = strip_prefix_ignore_case(text, prefix) {
            text = rest;
            break;
        }
    }
    if let Some(rest) = strip_prefix_ignore_case(text, "pmid") {
        text = rest
            .trim_start()
            .trim_start_matches(':')
            .trim_start()
            .trim_start_matches('#')
            .trim_start();
    }
    let digits = text.trim().trim_end_matches('/').trim();
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let canonical = digits.trim_start_matches('0');
    (!canonical.is_empty()).then(|| canonical.to_owned())
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        text.get(prefix.len()..)
    } else {
        None
    }
}

/// Parses an `efetch` answer into articles. Anything that is not a PubMed answer is a
/// retryable error, so it is never mistaken for "none of these were found".
pub fn parse_efetch(xml: &str) -> Result<Vec<PubmedArticle>, PubmedError> {
    if !xml.contains("<PubmedArticleSet") {
        return Err(PubmedError::Retryable(UNREADABLE.to_owned()));
    }
    Ok(parse_articles(xml)
        .into_iter()
        .map(Article::finish)
        .collect())
}

fn non_empty(text: String) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Form-encodes one query value the way the E-utilities expect.
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

#[derive(Debug, Default)]
struct Article {
    pmid: String,
    doi: String,
    title: String,
    abstract_text: String,
    journal: String,
    year: Option<i32>,
    authors: Vec<String>,
}

impl Article {
    fn finish(self) -> PubmedArticle {
        PubmedArticle {
            pmid: non_empty(self.pmid),
            doi: non_empty(self.doi).map(|doi| doi.to_lowercase()),
            title: non_empty(self.title),
            abstract_text: non_empty(self.abstract_text),
            journal: non_empty(self.journal),
            year: self.year,
            authors: self.authors,
        }
    }
}

/// Parse the PubMed `efetch` XML into articles.
fn parse_articles(xml: &str) -> Vec<Article> {
    let mut reader = Reader::from_str(xml);
    let mut articles = Vec::new();
    let mut current: Option<Article> = None;
    let mut path: Vec<String> = Vec::new();
    let mut doi_pending = false;
    let (mut last_name, mut fore_name) = (String::new(), String::new());
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                if name == "PubmedArticle" {
                    current = Some(Article::default());
                }
                if name == "ArticleId" {
                    doi_pending = element.attributes().flatten().any(|attribute| {
                        attribute.key.as_ref() == b"IdType" && attribute.value.as_ref() == b"doi"
                    });
                }
                if name == "Author" {
                    last_name.clear();
                    fore_name.clear();
                }
                path.push(name);
            }
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                path.pop();
                if name == "Author" {
                    let full = format!("{fore_name} {last_name}").trim().to_owned();
                    if let (Some(article), false) = (current.as_mut(), full.is_empty()) {
                        article.authors.push(full);
                    }
                }
                if name == "AbstractText"
                    && let Some(article) = current.as_mut()
                {
                    article.abstract_text.push(' ');
                }
                if name == "PubmedArticle"
                    && let Some(article) = current.take()
                {
                    articles.push(article);
                }
            }
            Ok(Event::Text(text)) => {
                if let (Some(article), Ok(text)) = (current.as_mut(), text.decode()) {
                    append_text(
                        article,
                        &path,
                        doi_pending,
                        &text,
                        &mut last_name,
                        &mut fore_name,
                    );
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                let resolved = match reference.decode().as_deref() {
                    Ok("amp") => "&",
                    Ok("lt") => "<",
                    Ok("gt") => ">",
                    Ok("quot") => "\"",
                    Ok("apos") => "'",
                    _ => "",
                };
                if let Some(article) = current.as_mut() {
                    append_text(
                        article,
                        &path,
                        doi_pending,
                        resolved,
                        &mut last_name,
                        &mut fore_name,
                    );
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    articles
}

fn append_text(
    article: &mut Article,
    path: &[String],
    doi_pending: bool,
    text: &str,
    last_name: &mut String,
    fore_name: &mut String,
) {
    let leaf = path.last().map_or("", String::as_str);
    let inside = |name: &str| path.iter().any(|segment| segment == name);
    match leaf {
        "PMID" if article.pmid.is_empty() && inside("MedlineCitation") => {
            article.pmid.push_str(text)
        }
        "ArticleTitle" => article.title.push_str(text),
        "AbstractText" => article.abstract_text.push_str(text),
        "ArticleId" if doi_pending => article.doi.push_str(text.trim()),
        "Title" if inside("Journal") => article.journal.push_str(text),
        "Year" if article.year.is_none() && (inside("PubDate") || inside("ArticleDate")) => {
            article.year = text.trim().parse().ok();
        }
        "LastName" if inside("Author") => last_name.push_str(text),
        "ForeName" if inside("Author") => fore_name.push_str(text),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/pubmed_efetch.xml");

    #[test]
    fn parses_articles_from_an_efetch_answer() {
        let articles = parse_efetch(FIXTURE).expect("the fixture is a PubMed answer");
        assert_eq!(articles.len(), 2);
        let first = &articles[0];
        assert_eq!(first.pmid.as_deref(), Some("123"));
        assert_eq!(first.doi.as_deref(), Some("10.1/abc"));
        assert_eq!(first.title.as_deref(), Some("Statins & hearts"));
        assert_eq!(first.journal.as_deref(), Some("The Lancet"));
        assert_eq!(first.year, Some(2024));
        assert_eq!(first.authors, ["Wei Li", "Ana Souza"]);
        assert_eq!(first.abstract_text.as_deref(), Some("Part one. Part two."));
    }

    #[test]
    fn leaves_missing_fields_empty_instead_of_inventing_them() {
        let articles = parse_efetch(FIXTURE).expect("the fixture is a PubMed answer");
        let bare = &articles[1];
        assert_eq!(bare.pmid.as_deref(), Some("456"));
        assert_eq!(bare.title.as_deref(), Some("A bare record"));
        assert_eq!(bare.doi, None);
        assert_eq!(bare.journal, None);
        assert_eq!(bare.year, None);
        assert_eq!(bare.abstract_text, None);
        assert!(bare.authors.is_empty());
    }

    #[test]
    fn an_empty_article_set_means_none_of_the_pmids_are_known() {
        let answer = "<?xml version=\"1.0\" ?><PubmedArticleSet></PubmedArticleSet>";
        assert_eq!(parse_efetch(answer), Ok(Vec::new()));
    }

    #[test]
    fn an_answer_that_is_not_pubmed_is_retryable() {
        let error = parse_efetch("<html>Service maintenance</html>")
            .expect_err("a web page is not a PubMed answer");
        assert!(error.is_retryable());
        assert_eq!(
            error.message(),
            "PubMed sent an answer that could not be understood."
        );
    }

    #[test]
    fn the_raw_record_keeps_identifiers_and_the_item_shape() {
        let article = parse_efetch(FIXTURE).expect("fixture").remove(0);
        let record = article.to_raw_record();
        let identifiers: Vec<(&str, &str)> = record
            .source_identifiers
            .iter()
            .map(|identifier| {
                (
                    identifier.scheme.as_str(),
                    identifier.normalized_value.as_str(),
                )
            })
            .collect();
        assert_eq!(identifiers, [("doi", "10.1/abc"), ("pmid", "123")]);
        assert_eq!(record.publication_year, Some(2024));
        assert_eq!(record.journal.as_deref(), Some("The Lancet"));
        assert_eq!(record.raw["source"], "pubmed");
        assert_eq!(record.raw["authors"][0], "Wei Li");
    }

    #[test]
    fn requests_carry_the_tool_name_and_the_workspace_contact_only() {
        let with_contact = PubmedClient::new(reqwest::Client::new(), " research@example.org ");
        let url = with_contact.url("efetch.fcgi", "db=pubmed&id=123");
        assert!(url.contains("&tool=DeepRef"), "{url}");
        assert!(url.ends_with("&email=research%40example.org"), "{url}");

        let without_contact = PubmedClient::new(reqwest::Client::new(), "   ");
        let url = without_contact.url("efetch.fcgi", "db=pubmed&id=123");
        assert!(url.contains("&tool=DeepRef"), "{url}");
        assert!(!url.contains("email="), "{url}");
    }

    #[test]
    fn reads_pmids_typed_as_numbers_labels_and_links() {
        for text in [
            "19446324",
            " 19446324 ",
            "PMID: 19446324",
            "pmid:19446324",
            "PMID #19446324",
            "https://pubmed.ncbi.nlm.nih.gov/19446324/",
            "pubmed.ncbi.nlm.nih.gov/19446324",
        ] {
            assert_eq!(normalize_pmid(text).as_deref(), Some("19446324"), "{text}");
        }
        assert_eq!(normalize_pmid("000123").as_deref(), Some("123"));
    }

    #[test]
    fn rejects_text_that_is_not_a_pmid() {
        for text in [
            "",
            "PMID",
            "0",
            "abc",
            "10.1000/xyz",
            "1234567890",
            "123 456",
            "https://example.org/123",
        ] {
            assert_eq!(normalize_pmid(text), None, "{text}");
        }
    }
}
