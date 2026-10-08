//! Optional enrichment of parsed PDFs with GROBID (section paths + references).
//!
//! Enrichment never fails a parse job: every error is downgraded to a warning
//! and the native result is kept.

use std::collections::HashMap;
use std::time::Duration;

use deepref_grobid::{GrobidClient, GrobidConfig, TeiDocument};
use deepref_postgres::{DocumentStructure, NewDocumentReference, NewDocumentSection};

const MIN_BODY_MATCH_CHARS: usize = 20;
const PREFIX_MATCH_CHARS: usize = 48;
const MIN_PREFIX_MATCH_CHARS: usize = 32;

/// Reads `GROBID_URL` (unset or blank disables enrichment) and validates it.
pub fn grobid_url_from_env() -> anyhow::Result<Option<String>> {
    let Some(url) = std::env::var("GROBID_URL")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    GrobidClient::new(GrobidConfig::new(url.clone()))
        .map_err(|error| anyhow::anyhow!("GROBID_URL is invalid: {error}"))?;
    Ok(Some(url))
}

pub fn grobid_client_from_env() -> anyhow::Result<Option<GrobidClient>> {
    let Some(url) = grobid_url_from_env()? else {
        return Ok(None);
    };
    let mut config = GrobidConfig::new(url);
    if let Some(secs) = std::env::var("GROBID_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
    {
        config.timeout = Duration::from_secs(secs);
    }
    Ok(Some(GrobidClient::new(config)?))
}

/// What the native parser produced for one block, as far as the merge cares.
#[derive(Debug, Clone, Copy)]
pub struct BlockView<'a> {
    pub ordinal: u32,
    pub text: &'a str,
    /// Role the native parser gave the block (`text`, `heading`, `title`, ...).
    pub native_role: &'a str,
    /// Section path the native parser already assigned (empty when none).
    pub native_path: &'a [String],
}

pub struct Enrichment {
    pub structure: DocumentStructure,
    pub parser_suffix: String,
}

/// Turns a TEI document into persistable structure for the given blocks.
pub fn build_structure(blocks: &[BlockView<'_>], tei: &TeiDocument) -> Enrichment {
    let section_paths = merge_section_paths(blocks, tei);
    let sections = tei
        .sections
        .iter()
        .filter(|section| {
            !section.title.is_empty() && !deepref_documents::is_caption_text(&section.title)
        })
        .map(|section| NewDocumentSection {
            number: section.number.clone(),
            title: section.title.clone(),
            depth: i32::try_from(section.depth).unwrap_or(i32::MAX),
            path: section.path.clone(),
            source: "grobid".to_owned(),
        })
        .collect();
    let references = tei
        .references
        .iter()
        .map(|reference| NewDocumentReference {
            raw: reference.raw.clone(),
            title: reference.title.clone(),
            authors: reference.authors.clone(),
            year: reference.year,
            venue: reference.venue.clone(),
            doi: reference.doi.clone(),
            source: "grobid".to_owned(),
        })
        .collect();
    let parser_suffix = match tei.grobid_version.as_deref() {
        Some(version) => format!("+grobid-{}", sanitize_version(version)),
        None => "+grobid".to_owned(),
    };
    Enrichment {
        structure: DocumentStructure {
            section_paths,
            sections,
            references,
        },
        parser_suffix,
    }
}

/// Structure derived by the native parser alone: its section paths and references.
pub fn native_structure(structured: &deepref_documents::StructuredDocument) -> DocumentStructure {
    let section_paths = structured
        .document
        .blocks
        .iter()
        .zip(&structured.block_structure)
        .filter(|(_, native)| !native.section_path.is_empty())
        .map(|(block, native)| (block.ordinal, native.section_path.clone()))
        .collect();
    let references = structured
        .references
        .iter()
        .map(|reference| NewDocumentReference {
            raw: reference.raw_text.clone(),
            title: None,
            authors: Vec::new(),
            year: reference.year.map(i32::from),
            venue: None,
            doi: reference.doi.clone(),
            source: "native".to_owned(),
        })
        .collect();
    DocumentStructure {
        section_paths,
        sections: Vec::new(),
        references,
    }
}

/// Combines GROBID output (already limited to blocks where it is empty/deeper)
/// with the native structure: native paths stay unless GROBID overrode them,
/// and GROBID's bibliography replaces the native one only when it found entries.
pub fn overlay_on_native(
    grobid: &mut DocumentStructure,
    structured: &deepref_documents::StructuredDocument,
) {
    let mut paths = native_structure(structured).section_paths;
    paths.extend(std::mem::take(&mut grobid.section_paths));
    grobid.section_paths = paths;
    if grobid.references.is_empty() {
        grobid.references = native_structure(structured).references;
    }
}

fn sanitize_version(version: &str) -> String {
    version
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(24)
        .collect()
}

fn normalize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for ch in value.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// Maps block ordinals to the GROBID section path, only where the native path
/// is empty or GROBID's is strictly deeper.
pub fn merge_section_paths(
    blocks: &[BlockView<'_>],
    tei: &TeiDocument,
) -> HashMap<u32, Vec<String>> {
    let mut corpus = String::new();
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let mut headings: HashMap<String, usize> = HashMap::new();
    for (index, section) in tei.sections.iter().enumerate() {
        let title = normalize(&section.title);
        if !title.is_empty() {
            headings.entry(title.clone()).or_insert(index);
            if let Some(number) = &section.number {
                headings
                    .entry(normalize(&format!("{number} {title}")))
                    .or_insert(index);
            }
        }
        let body = normalize(&section.text);
        if body.is_empty() {
            continue;
        }
        if !corpus.is_empty() {
            corpus.push_str(" | ");
        }
        starts.push((corpus.len(), index));
        corpus.push_str(&body);
    }

    let section_at = |offset: usize| -> Option<usize> {
        let position = starts.partition_point(|(start, _)| *start <= offset);
        position.checked_sub(1).map(|i| starts[i].1)
    };

    let mut result = HashMap::new();
    let mut cursor = 0_usize;
    for block in blocks {
        let text = normalize(block.text);
        if text.is_empty() {
            continue;
        }
        let matched = if let Some(index) = headings.get(&text) {
            Some(*index)
        } else if text.len() >= MIN_BODY_MATCH_CHARS {
            find_in_corpus(&corpus, &text, cursor)
                .inspect(|offset| cursor = *offset)
                .and_then(section_at)
        } else {
            None
        };
        let Some(index) = matched else { continue };
        // Front matter belongs to no section, and a caption keeps the section it sits in.
        if matches!(block.native_role, "title" | "front_matter" | "caption") {
            continue;
        }
        let path = &tei.sections[index].path;
        if path.is_empty() {
            continue;
        }
        let more_specific = block.native_path.is_empty() || path.len() > block.native_path.len();
        if more_specific {
            result.insert(block.ordinal, path.clone());
        }
    }
    result
}

/// Containment first (from the reading-order cursor, then anywhere), then a
/// fuzzy fallback on the block's leading and trailing characters.
fn find_in_corpus(corpus: &str, text: &str, cursor: usize) -> Option<usize> {
    let find_from = |needle: &str| -> Option<usize> {
        corpus
            .get(cursor..)
            .and_then(|tail| tail.find(needle))
            .map(|offset| cursor + offset)
            .or_else(|| corpus.find(needle))
    };
    if let Some(offset) = find_from(text) {
        return Some(offset);
    }
    let char_count = text.chars().count();
    if char_count < MIN_PREFIX_MATCH_CHARS {
        return None;
    }
    let prefix: String = text.chars().take(PREFIX_MATCH_CHARS).collect();
    if let Some(offset) = find_from(&prefix) {
        return Some(offset);
    }
    let suffix: String = text
        .chars()
        .skip(char_count.saturating_sub(PREFIX_MATCH_CHARS))
        .collect();
    find_from(&suffix)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use deepref_grobid::{TeiReference, TeiSection};

    fn tei() -> TeiDocument {
        let section = |number: &str, title: &str, path: &[&str], text: &str| TeiSection {
            number: Some(number.to_owned()),
            title: title.to_owned(),
            depth: path.len(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            text: text.to_owned(),
        };
        TeiDocument {
            grobid_version: Some("0.8.2".to_owned()),
            sections: vec![
                section(
                    "1",
                    "Introduction",
                    &["Introduction"],
                    "Stroke is a leading cause of disability worldwide and recurrence is common.",
                ),
                section(
                    "2",
                    "Methods",
                    &["Methods"],
                    "We ran a randomised double blind trial.",
                ),
                section(
                    "2.1",
                    "Participants",
                    &["Methods", "Participants"],
                    "Adults aged 40 to 80 were recruited from five hospitals.",
                ),
            ],
            references: vec![TeiReference {
                ordinal: 1,
                raw: "Doe J. Aspirin. Stroke. 2019.".to_owned(),
                title: Some("Aspirin".to_owned()),
                authors: vec!["Jane Doe".to_owned()],
                year: Some(2019),
                venue: Some("Stroke".to_owned()),
                doi: None,
            }],
            ..Default::default()
        }
    }

    fn block<'a>(ordinal: u32, text: &'a str, native: &'a [String]) -> BlockView<'a> {
        BlockView {
            ordinal,
            text,
            native_role: "text",
            native_path: native,
        }
    }

    #[test]
    fn front_matter_and_captions_never_take_a_grobid_path() {
        let none: Vec<String> = Vec::new();
        let blocks = [
            BlockView {
                ordinal: 0,
                text: "Adults aged 40 to 80 were recruited from five hospitals.",
                native_role: "title",
                native_path: &none,
            },
            BlockView {
                ordinal: 1,
                text: "Adults aged 40 to 80 were recruited from five hospitals.",
                native_role: "caption",
                native_path: &none,
            },
            BlockView {
                ordinal: 2,
                text: "Adults aged 40 to 80 were recruited from five hospitals.",
                native_role: "text",
                native_path: &none,
            },
        ];
        let paths = merge_section_paths(&blocks, &tei());
        assert!(!paths.contains_key(&0));
        assert!(!paths.contains_key(&1));
        assert_eq!(paths[&2], ["Methods", "Participants"]);
    }

    #[test]
    fn caption_sections_from_grobid_are_not_listed_as_sections() {
        let mut document = tei();
        document.sections.push(TeiSection {
            number: None,
            title: "TABLE 3. Linear Regression Model for Independent Predictors".to_owned(),
            depth: 1,
            path: vec!["TABLE 3. Linear Regression Model for Independent Predictors".to_owned()],
            text: String::new(),
        });
        let enrichment = build_structure(&[], &document);
        assert_eq!(enrichment.structure.sections.len(), 3);
    }

    #[test]
    fn matches_headings_and_body_text() {
        let none: Vec<String> = Vec::new();
        let blocks = [
            block(0, "1 Introduction", &none),
            block(
                1,
                "Stroke is a leading cause of disability worldwide, and recurrence is common.",
                &none,
            ),
            block(2, "2.1. Participants", &none),
            block(
                3,
                "Adults aged 40 to 80 were recruited from five hospitals.",
                &none,
            ),
            block(4, "42", &none),
            block(
                5,
                "Entirely unrelated text that GROBID never saw in the document.",
                &none,
            ),
        ];
        let paths = merge_section_paths(&blocks, &tei());
        assert_eq!(paths[&0], ["Introduction"]);
        assert_eq!(paths[&1], ["Introduction"]);
        assert_eq!(paths[&2], ["Methods", "Participants"]);
        assert_eq!(paths[&3], ["Methods", "Participants"]);
        assert!(!paths.contains_key(&4));
        assert!(!paths.contains_key(&5));
    }

    #[test]
    fn prefix_fallback_handles_hyphenation_differences() {
        let none: Vec<String> = Vec::new();
        let blocks = [block(
            0,
            "Stroke is a leading cause of disability worldwide and recurrence is com- mon in practice.",
            &none,
        )];
        let paths = merge_section_paths(&blocks, &tei());
        assert_eq!(paths[&0], ["Introduction"]);
    }

    #[test]
    fn keeps_native_path_unless_grobid_is_deeper() {
        let shallow = vec!["Methods".to_owned()];
        let same = vec!["Introduction".to_owned()];
        let blocks = [
            block(
                0,
                "Adults aged 40 to 80 were recruited from five hospitals.",
                &shallow,
            ),
            block(
                1,
                "Stroke is a leading cause of disability worldwide and recurrence is common.",
                &same,
            ),
        ];
        let paths = merge_section_paths(&blocks, &tei());
        assert_eq!(paths[&0], ["Methods", "Participants"]);
        assert!(!paths.contains_key(&1));
    }

    #[test]
    fn build_structure_records_version_suffix_and_references() {
        let enrichment = build_structure(&[], &tei());
        assert_eq!(enrichment.parser_suffix, "+grobid-0.8.2");
        assert_eq!(enrichment.structure.sections.len(), 3);
        assert_eq!(enrichment.structure.references.len(), 1);
        assert_eq!(enrichment.structure.references[0].source, "grobid");
    }
}
