//! Does this PDF belong to the report it is attached to?
//!
//! After a parse, the title and DOI printed on the PDF are compared with the report's. The
//! result is advice: a mismatch is flagged on the document and never blocks an upload.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::structure::extract_doi;
use crate::{ParsedBlock, StructuredDocument};

/// Share of the report title's content words that must appear in the PDF's title.
const TITLE_OVERLAP_MATCH: f64 = 0.5;
/// Titles with fewer content words than this are too generic to compare.
const MIN_TITLE_WORDS: usize = 3;
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "into", "over", "between", "among",
    "during", "after", "its", "their", "are", "was", "were", "has", "have", "been", "using", "uma",
    "com", "para", "dos", "das", "nos", "nas", "que", "por", "sobre", "entre", "como", "mais",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityVerdict {
    Match,
    Mismatch,
    Unknown,
}

/// The outcome of comparing a parsed PDF with its report, stored with the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityCheck {
    pub verdict: IdentityVerdict,
    /// The title the PDF states about itself, for the researcher to read.
    pub detected_title: Option<String>,
    /// The DOI printed on the PDF's first page (or in its GROBID header), lower-cased.
    pub detected_doi: Option<String>,
    /// Share of the report title's content words found in the PDF's title, 0.0 to 1.0.
    pub title_overlap: Option<f64>,
}

/// What a parsed PDF says about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfIdentity {
    /// GROBID's title, else the title block on page 1.
    pub title: Option<String>,
    /// GROBID's DOI, else the first DOI on page 1.
    pub doi: Option<String>,
    /// Page-1 text, compared only when no title was found.
    pub first_page: String,
}

/// The report fields the PDF is compared with.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReportIdentity<'a> {
    pub title: Option<&'a str>,
    pub doi: Option<&'a str>,
}

impl PdfIdentity {
    /// Reads the identity from the native parse; `header_title` and `header_doi` come from GROBID
    /// when it ran. Parse blocks and their roles are parallel, so the title block is the first
    /// `title` role on page 1.
    pub fn from_parse(
        structured: &StructuredDocument,
        header_title: Option<&str>,
        header_doi: Option<&str>,
    ) -> Self {
        let page_one: Vec<(&ParsedBlock, &str)> = structured
            .document
            .blocks
            .iter()
            .zip(&structured.block_structure)
            .filter(|(block, _)| block.page_number == 1)
            .map(|(block, structure)| (block, structure.role.as_str()))
            .collect();
        let first_page = page_one
            .iter()
            .map(|(block, _)| block.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let title_block = page_one
            .iter()
            .filter(|(_, role)| *role == "title")
            .map(|(block, _)| block.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let title = non_empty(header_title)
            .or_else(|| non_empty(Some(title_block.as_str())))
            .map(str::to_owned);
        let doi = header_doi
            .map(|doi| doi.trim().to_lowercase())
            .filter(|doi| !doi.is_empty())
            .or_else(|| extract_doi(&first_page));
        Self {
            title,
            doi,
            first_page,
        }
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Compares the PDF with the report. A DOI that matches settles it. Otherwise the report title's
/// content words decide: at least half must appear in the PDF's title (or, when the PDF has no
/// title, on its first page). A DOI that differs is evidence only when no title can be compared.
pub fn check_identity(pdf: &PdfIdentity, report: &ReportIdentity<'_>) -> IdentityCheck {
    let report_doi = report.doi.map(doi_key).filter(|doi| !doi.is_empty());
    let pdf_doi = pdf
        .doi
        .as_deref()
        .map(doi_key)
        .filter(|doi| !doi.is_empty());
    let doi_match = matches!((&pdf_doi, &report_doi), (Some(a), Some(b)) if a == b);
    let doi_conflict = matches!((&pdf_doi, &report_doi), (Some(a), Some(b)) if a != b);

    let report_words = report.title.map(content_words).unwrap_or_default();
    let overlap = if report_words.len() < MIN_TITLE_WORDS {
        None
    } else {
        let haystack = match pdf.title.as_deref() {
            Some(title) => content_words(title),
            None => content_words(&pdf.first_page),
        };
        (!haystack.is_empty()).then(|| {
            let hits = report_words
                .iter()
                .filter(|word| haystack.contains(*word))
                .count();
            hits as f64 / report_words.len() as f64
        })
    };

    let verdict = if doi_match {
        IdentityVerdict::Match
    } else {
        match overlap {
            Some(share) if share >= TITLE_OVERLAP_MATCH => IdentityVerdict::Match,
            Some(_) => IdentityVerdict::Mismatch,
            None if doi_conflict => IdentityVerdict::Mismatch,
            None => IdentityVerdict::Unknown,
        }
    };
    IdentityCheck {
        verdict,
        detected_title: pdf.title.clone(),
        detected_doi: pdf_doi,
        title_overlap: overlap.map(|share| (share * 100.0).round() / 100.0),
    }
}

fn doi_key(doi: &str) -> String {
    let value = doi.trim().to_lowercase();
    value
        .strip_prefix("https://doi.org/")
        .or_else(|| value.strip_prefix("http://doi.org/"))
        .or_else(|| value.strip_prefix("doi:"))
        .unwrap_or(&value)
        .trim()
        .to_owned()
}

/// Distinct lower-case words of 3+ characters without accents or stop words.
fn content_words(text: &str) -> std::collections::BTreeSet<String> {
    let folded: String = text
        .nfkd()
        .filter(|character| !('\u{300}'..='\u{36f}').contains(character))
        .collect::<String>()
        .to_lowercase();
    folded
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3 && !STOP_WORDS.contains(word))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(title: Option<&str>, doi: Option<&str>, first_page: &str) -> PdfIdentity {
        PdfIdentity {
            title: title.map(str::to_owned),
            doi: doi.map(str::to_owned),
            first_page: first_page.to_owned(),
        }
    }

    const FITBIT: &str =
        "Randomized Trial of a Fitbit-Based Physical Activity Intervention for Adults";

    #[test]
    fn a_stroke_paper_on_a_fitbit_report_is_a_mismatch() {
        let check = check_identity(
            &pdf(
                Some(
                    "Inpatient rehabilitation outcomes after stroke: a randomized trial of early mobilisation",
                ),
                None,
                "",
            ),
            &ReportIdentity {
                title: Some(FITBIT),
                doi: Some("10.1016/j.amepre.2015.01.020"),
            },
        );
        assert_eq!(check.verdict, IdentityVerdict::Mismatch);
        assert!(check.title_overlap.unwrap() < 0.5);
    }

    #[test]
    fn the_same_article_matches_by_title_or_doi() {
        let by_title = check_identity(
            &pdf(
                Some(
                    "A Randomized Trial of a Fitbit-Based Physical Activity Intervention for Adults",
                ),
                None,
                "",
            ),
            &ReportIdentity {
                title: Some(FITBIT),
                doi: Some("10.1016/x"),
            },
        );
        assert_eq!(by_title.verdict, IdentityVerdict::Match);
        let by_doi = check_identity(
            &pdf(
                Some("Unrelated banner"),
                Some("https://doi.org/10.1016/X"),
                "",
            ),
            &ReportIdentity {
                title: Some(FITBIT),
                doi: Some("10.1016/x"),
            },
        );
        assert_eq!(by_doi.verdict, IdentityVerdict::Match);
    }

    #[test]
    fn a_preprint_with_another_doi_but_the_same_title_still_matches() {
        let check = check_identity(
            &pdf(Some(FITBIT), Some("10.48550/arxiv.1"), ""),
            &ReportIdentity {
                title: Some(FITBIT),
                doi: Some("10.1016/x"),
            },
        );
        assert_eq!(check.verdict, IdentityVerdict::Match);
    }

    #[test]
    fn a_differing_doi_alone_is_a_mismatch_only_without_a_comparable_title() {
        let report = ReportIdentity {
            title: None,
            doi: Some("10.1000/report"),
        };
        let check = check_identity(&pdf(None, Some("10.1000/other"), ""), &report);
        assert_eq!(check.verdict, IdentityVerdict::Mismatch);
        assert_eq!(check.detected_doi.as_deref(), Some("10.1000/other"));
    }

    #[test]
    fn without_evidence_the_check_is_unknown() {
        let report = ReportIdentity {
            title: Some(FITBIT),
            doi: None,
        };
        assert_eq!(
            check_identity(&pdf(None, None, ""), &report).verdict,
            IdentityVerdict::Unknown
        );
        let generic = ReportIdentity {
            title: Some("Stroke"),
            doi: None,
        };
        assert_eq!(
            check_identity(&pdf(Some("Anything"), None, ""), &generic).verdict,
            IdentityVerdict::Unknown
        );
    }

    #[test]
    fn accents_and_stop_words_do_not_count() {
        let words = content_words("Atividade Física e Saúde: a trial of the 2024 study");
        assert!(words.contains("atividade"));
        assert!(words.contains("fisica"));
        assert!(words.contains("saude"));
        assert!(!words.contains("the"));
        assert!(!words.contains("and"));
    }
}
