//! Provider-agnostic inbound e-mail: one normalized shape plus adapters for
//! the Postmark and Mailgun inbound payloads, and DOI / PubMed ID extraction
//! from the message body.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

/// Longest message text kept as trigger data.
pub const MAX_EMAIL_TEXT_CHARS: usize = 200_000;
pub const MAX_EMAIL_ATTACHMENTS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InboundAttachment {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InboundEmail {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub html: String,
    #[serde(default)]
    pub attachments: Vec<InboundAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InboundEmailError {
    #[error("the message has no sender")]
    MissingSender,
    #[error("the message is empty")]
    Empty,
    #[error("the message format is not recognised")]
    Unrecognised,
}

impl InboundEmail {
    /// Trim sizes and reject empty messages.
    pub fn normalized(mut self) -> Result<Self, InboundEmailError> {
        if self.from.trim().is_empty() {
            return Err(InboundEmailError::MissingSender);
        }
        if self.text.trim().is_empty()
            && self.html.trim().is_empty()
            && self.subject.trim().is_empty()
        {
            return Err(InboundEmailError::Empty);
        }
        self.text = self.text.chars().take(MAX_EMAIL_TEXT_CHARS).collect();
        self.html = self.html.chars().take(MAX_EMAIL_TEXT_CHARS).collect();
        self.attachments.truncate(MAX_EMAIL_ATTACHMENTS);
        Ok(self)
    }

    /// The readable body: the text part, or the HTML with tags removed.
    pub fn body_text(&self) -> String {
        if !self.text.trim().is_empty() {
            return self.text.clone();
        }
        strip_html(&self.html)
    }

    /// Trigger data handed to the workflow. DOIs and PubMed IDs found in the
    /// subject and body are listed in `dois`, `pmids` and `identifiers`.
    pub fn trigger_data(&self) -> Value {
        let body = self.body_text();
        let found = extract_identifiers(&format!("{}\n{}", self.subject, body));
        let identifiers: Vec<Value> = found
            .dois
            .iter()
            .map(|doi| json!({ "scheme": "doi", "value": doi }))
            .chain(
                found
                    .pmids
                    .iter()
                    .map(|pmid| json!({ "scheme": "pmid", "value": pmid })),
            )
            .collect();
        json!({
            "from": self.from,
            "to": self.to,
            "subject": self.subject,
            "text": body,
            "html": self.html,
            "attachments": self.attachments,
            "dois": found.dois,
            "pmids": found.pmids,
            "identifiers": identifiers,
        })
    }
}

fn strip_html(html: &str) -> String {
    static TAGS: OnceLock<Option<Regex>> = OnceLock::new();
    let tags = TAGS.get_or_init(|| Regex::new(r"(?s)<[^>]*>").ok());
    match tags {
        Some(tags) => tags.replace_all(html, " ").into_owned(),
        None => html.to_owned(),
    }
}

fn text_field(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}

/// Adapt a Postmark inbound webhook body.
pub fn from_postmark(payload: &Value) -> Result<InboundEmail, InboundEmailError> {
    if payload.get("TextBody").is_none()
        && payload.get("HtmlBody").is_none()
        && payload.get("FromFull").is_none()
    {
        return Err(InboundEmailError::Unrecognised);
    }
    let from = payload
        .pointer("/FromFull/Email")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| text_field(payload, &["From"]));
    let to = payload
        .get("ToFull")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry.get("Email").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|joined| !joined.is_empty())
        .unwrap_or_else(|| text_field(payload, &["To"]));
    let attachments = payload
        .get("Attachments")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|attachment| InboundAttachment {
                    filename: text_field(attachment, &["Name"]),
                    content_type: text_field(attachment, &["ContentType"]),
                    size: attachment
                        .get("ContentLength")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    InboundEmail {
        from,
        to,
        subject: text_field(payload, &["Subject"]),
        text: text_field(payload, &["TextBody"]),
        html: text_field(payload, &["HtmlBody"]),
        attachments,
    }
    .normalized()
}

/// Adapt a Mailgun inbound route body (form fields, as a JSON object).
pub fn from_mailgun(fields: &Value) -> Result<InboundEmail, InboundEmailError> {
    if fields.get("body-plain").is_none()
        && fields.get("body-html").is_none()
        && fields.get("stripped-text").is_none()
    {
        return Err(InboundEmailError::Unrecognised);
    }
    let count = fields
        .get("attachment-count")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or_default();
    let attachments = (1..=count.min(MAX_EMAIL_ATTACHMENTS as u64))
        .map(|index| InboundAttachment {
            filename: format!("attachment-{index}"),
            ..InboundAttachment::default()
        })
        .collect();
    let text = {
        let plain = text_field(fields, &["body-plain"]);
        if plain.is_empty() {
            text_field(fields, &["stripped-text"])
        } else {
            plain
        }
    };
    InboundEmail {
        from: text_field(fields, &["from", "sender"]),
        to: text_field(fields, &["To", "to", "recipient"]),
        subject: text_field(fields, &["subject", "Subject"]),
        text,
        html: text_field(fields, &["body-html"]),
        attachments,
    }
    .normalized()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FoundIdentifiers {
    pub dois: Vec<String>,
    pub pmids: Vec<String>,
}

/// Find DOIs and PubMed IDs in free text. DOIs are lower-cased, PubMed IDs
/// are digits only; both are de-duplicated in order of appearance.
pub fn extract_identifiers(text: &str) -> FoundIdentifiers {
    static DOI: OnceLock<Option<Regex>> = OnceLock::new();
    static PMID: OnceLock<Option<Regex>> = OnceLock::new();
    let doi = DOI.get_or_init(|| Regex::new(r#"(?i)\b10\.\d{4,9}/[^\s"'<>]+"#).ok());
    let pmid = PMID.get_or_init(|| {
        Regex::new(r"(?i)(?:\bpmid\b[:\s#]*|pubmed(?:\.ncbi\.nlm\.nih\.gov)?/)(\d{1,9})\b").ok()
    });
    let mut found = FoundIdentifiers::default();
    if let Some(doi) = doi {
        for hit in doi.find_iter(text) {
            let cleaned = hit
                .as_str()
                .trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '>', '\'', '"'])
                .to_lowercase();
            if cleaned.len() > 7 && !found.dois.contains(&cleaned) {
                found.dois.push(cleaned);
            }
        }
    }
    if let Some(pmid) = pmid {
        for captures in pmid.captures_iter(text) {
            if let Some(id) = captures.get(1) {
                let id = id.as_str().trim_start_matches('0').to_owned();
                if !id.is_empty() && !found.pmids.contains(&id) {
                    found.pmids.push(id);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_cleans_identifiers() {
        let text = "See https://doi.org/10.1000/ABC.123, and (10.5555/xyz-9). Also PMID: 31452104 and https://pubmed.ncbi.nlm.nih.gov/12345678/ plus PMID 31452104 again.";
        let found = extract_identifiers(text);
        assert_eq!(found.dois, vec!["10.1000/abc.123", "10.5555/xyz-9"]);
        assert_eq!(found.pmids, vec!["31452104", "12345678"]);
    }

    #[test]
    fn postmark_payload_is_adapted() {
        let payload = json!({
            "FromFull": {"Email": "alerts@journal.org", "Name": "Alerts"},
            "ToFull": [{"Email": "abc@in.deepref.app"}],
            "Subject": "New paper",
            "TextBody": "Read 10.1234/foo.bar now",
            "HtmlBody": "<p>Read</p>",
            "Attachments": [{"Name": "a.pdf", "ContentType": "application/pdf", "ContentLength": 10, "Content": "AAAA"}]
        });
        let email = from_postmark(&payload).expect("postmark");
        assert_eq!(email.from, "alerts@journal.org");
        assert_eq!(email.to, "abc@in.deepref.app");
        assert_eq!(email.attachments[0].filename, "a.pdf");
        let data = email.trigger_data();
        assert_eq!(data["dois"][0], "10.1234/foo.bar");
        assert_eq!(data["identifiers"][0]["scheme"], "doi");
    }

    #[test]
    fn mailgun_payload_is_adapted() {
        let payload = json!({
            "sender": "bob@example.org",
            "recipient": "abc@in.deepref.app",
            "subject": "pm",
            "body-plain": "PMID: 99887766",
            "attachment-count": "2"
        });
        let email = from_mailgun(&payload).expect("mailgun");
        assert_eq!(email.attachments.len(), 2);
        assert_eq!(email.trigger_data()["pmids"][0], "99887766");
    }

    #[test]
    fn unrecognised_and_empty_messages_are_rejected() {
        assert_eq!(
            from_postmark(&json!({"x": 1})),
            Err(InboundEmailError::Unrecognised)
        );
        assert_eq!(
            from_mailgun(&json!({"x": 1})),
            Err(InboundEmailError::Unrecognised)
        );
        assert_eq!(
            InboundEmail::default().normalized(),
            Err(InboundEmailError::MissingSender)
        );
        let html_only = InboundEmail {
            from: "a@b.c".into(),
            html: "<b>10.1111/x.y</b>".into(),
            ..InboundEmail::default()
        }
        .normalized()
        .expect("html only is fine");
        assert_eq!(html_only.trigger_data()["dois"][0], "10.1111/x.y");
    }
}
