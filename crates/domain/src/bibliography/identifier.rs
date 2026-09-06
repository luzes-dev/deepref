use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierScheme {
    Doi,
    Pmid,
    Pmcid,
    Arxiv,
    Isbn,
    ClinicalTrialRegistry,
    Other,
}

impl IdentifierScheme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Doi => "doi",
            Self::Pmid => "pmid",
            Self::Pmcid => "pmcid",
            Self::Arxiv => "arxiv",
            Self::Isbn => "isbn",
            Self::ClinicalTrialRegistry => "clinical_trial_registry",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for IdentifierScheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DoiError {
    #[error("DOI is empty")]
    Empty,
    #[error("DOI must start with 10. and include a suffix")]
    InvalidShape,
}

/// Normalize DOI spellings at the domain boundary. This is the sole DOI
/// normalization implementation used by the workspace.
pub fn normalize_doi(input: &str) -> Result<String, DoiError> {
    let mut value: String = input.nfkc().collect::<String>().trim().to_lowercase();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ] {
        if let Some(stripped) = value.strip_prefix(prefix) {
            value = stripped.to_owned();
            break;
        }
    }

    value = value.trim().trim_matches('.').to_owned();
    if value.is_empty() {
        return Err(DoiError::Empty);
    }
    if !value.starts_with("10.") || !value.contains('/') {
        return Err(DoiError::InvalidShape);
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentifierError {
    #[error("identifier value is empty")]
    Empty,
    #[error("invalid DOI: {0}")]
    Doi(#[from] DoiError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportIdentifier {
    pub scheme: IdentifierScheme,
    pub original: String,
    pub normalized: String,
}

impl ReportIdentifier {
    pub fn new(
        scheme: IdentifierScheme,
        original: impl Into<String>,
    ) -> Result<Self, IdentifierError> {
        let original = original.into();
        let normalized = match scheme {
            IdentifierScheme::Doi => normalize_doi(&original)?,
            _ => normalize_value(&original)?,
        };
        Ok(Self {
            scheme,
            original,
            normalized,
        })
    }
}

fn normalize_value(input: &str) -> Result<String, IdentifierError> {
    let normalized: String = input.nfkc().collect::<String>().trim().to_lowercase();
    if normalized.is_empty() {
        return Err(IdentifierError::Empty);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_doi_forms_with_one_implementation() {
        assert_eq!(
            normalize_doi(" https://doi.org/10.1145/1234.5678. ").unwrap(),
            "10.1145/1234.5678"
        );
        assert_eq!(normalize_doi("doi:10.1000/XYZ").unwrap(), "10.1000/xyz");
        assert_eq!(
            ReportIdentifier::new(IdentifierScheme::Doi, "DOI:10.1000/XYZ")
                .unwrap()
                .normalized,
            "10.1000/xyz"
        );
        assert_eq!(normalize_doi("").unwrap_err(), DoiError::Empty);
        assert_eq!(
            normalize_doi("not-a-doi").unwrap_err(),
            DoiError::InvalidShape
        );
    }

    #[test]
    fn normalizes_unicode_equivalent_identifier_values() {
        let composed = ReportIdentifier::new(IdentifierScheme::Pmid, "PMID: Café").unwrap();
        let decomposed =
            ReportIdentifier::new(IdentifierScheme::Pmid, "ＰＭＩＤ: Cafe\u{301}").unwrap();
        assert_eq!(composed.normalized, decomposed.normalized);
    }

    #[test]
    fn keeps_original_identifier_value() {
        let identifier =
            ReportIdentifier::new(IdentifierScheme::Isbn, " 978-0-00-000000-0 ").unwrap();
        assert_eq!(identifier.original, " 978-0-00-000000-0 ");
        assert_eq!(identifier.normalized, "978-0-00-000000-0");
    }

    proptest::proptest! {
        #[test]
        fn doi_normalization_is_idempotent(input in proptest::prelude::any::<String>()) {
            if let Ok(normalized) = normalize_doi(&input) {
                let second = normalize_doi(&normalized).expect("idempotent re-normalization must succeed");
                proptest::prop_assert_eq!(second, normalized);
            }
        }

        #[test]
        fn doi_normalization_satisfies_invariants(input in proptest::prelude::any::<String>()) {
            if let Ok(normalized) = normalize_doi(&input) {
                proptest::prop_assert!(normalized.starts_with("10."));
                proptest::prop_assert!(normalized.contains("/"));
                proptest::prop_assert_eq!(normalized.to_lowercase(), normalized.clone());
                proptest::prop_assert!(!normalized.starts_with(".") && !normalized.ends_with("."));
                proptest::prop_assert_eq!(normalized.trim(), normalized.as_str());
            }
        }

        #[test]
        fn doi_normalization_handles_arbitrary_valid_doi(
            prefix_choice in 0..6usize,
            suffix in "[a-zA-Z0-9._-]{1,30}/[a-zA-Z0-9._-]{1,50}",
            trailing_dots in 0..4usize,
        ) {
            let base = format!("10.{suffix}");
            let prefixes = [
                "",
                "https://doi.org/",
                "http://doi.org/",
                "https://dx.doi.org/",
                "http://dx.doi.org/",
                "doi:",
            ];
            let raw = format!("{}{}{}", prefixes[prefix_choice], base, ".".repeat(trailing_dots));
            if let Ok(normalized) = normalize_doi(&raw) {
                proptest::prop_assert_eq!(normalize_doi(&normalized).unwrap(), normalized.clone());
                proptest::prop_assert!(normalized.starts_with("10."));
            }
        }
    }
}
