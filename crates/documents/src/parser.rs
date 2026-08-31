use std::{path::Path, time::Instant};

use deepref_domain::NormalizedBoundingBox;
use pdfium_render::prelude::{Pdfium, PdfiumError};
use thiserror::Error;

use crate::{ParsedBlock, ParsedDocument, ParsedPage, content_sha256};

pub trait DocumentParser: Send + Sync {
    fn version(&self) -> &'static str;
    fn parse_file(&self, path: &Path) -> Result<ParsedDocument, PdfParserError>;
}

#[derive(Debug, Clone, Copy)]
pub struct ParserLimits {
    pub max_pages: usize,
    pub max_blocks: usize,
    pub max_text_bytes: usize,
    pub max_duration: std::time::Duration,
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self {
            max_pages: 500,
            max_blocks: 50_000,
            max_text_bytes: 10 * 1024 * 1024,
            max_duration: std::time::Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Error)]
pub enum PdfParserError {
    #[error("Pdfium could not be loaded: {0}")]
    Library(String),
    #[error("Pdfium could not open the document: {0}")]
    Document(String),
    #[error("Pdfium document exceeded parser limits: {0}")]
    Limit(&'static str),
}

pub struct PdfiumParser {
    pdfium: Pdfium,
    limits: ParserLimits,
}

impl PdfiumParser {
    pub fn from_config(
        library_path: Option<&Path>,
        limits: ParserLimits,
    ) -> Result<Self, PdfParserError> {
        let bindings = match library_path {
            Some(path) => Pdfium::bind_to_library(path),
            None => Pdfium::bind_to_system_library(),
        };
        if matches!(
            bindings,
            Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized)
        ) {
            return Ok(Self {
                pdfium: Pdfium::default(),
                limits,
            });
        }
        let bindings = bindings.map_err(|error| PdfParserError::Library(error.to_string()))?;
        Ok(Self {
            pdfium: Pdfium::new(bindings),
            limits,
        })
    }

    pub fn from_env() -> Result<Self, PdfParserError> {
        let library_path = std::env::var_os("PDFIUM_LIBRARY_PATH");
        Self::from_config(
            library_path.as_deref().map(Path::new),
            ParserLimits::default(),
        )
    }

    pub fn parse_file(&self, path: &Path) -> Result<ParsedDocument, PdfParserError> {
        let started = Instant::now();
        let document = self
            .pdfium
            .load_pdf_from_file(path, None)
            .map_err(|error| PdfParserError::Document(error.to_string()))?;
        let page_count = document.pages().len();
        if page_count > self.limits.max_pages as i32 {
            return Err(PdfParserError::Limit("page count"));
        }
        let mut blocks = Vec::new();
        let mut pages = Vec::with_capacity(page_count as usize);
        let mut text_bytes = 0usize;
        for page_number in 0..page_count {
            if started.elapsed() > self.limits.max_duration {
                return Err(PdfParserError::Limit("parse duration"));
            }
            let page = document
                .pages()
                .get(page_number)
                .map_err(|error| PdfParserError::Document(error.to_string()))?;
            let page_width = page.width().value;
            let page_height = page.height().value;
            if !(page_width.is_finite() && page_height.is_finite())
                || page_width <= 0.0
                || page_height <= 0.0
            {
                continue;
            }
            let page_text = page
                .text()
                .map_err(|error| PdfParserError::Document(error.to_string()))?;
            let page_ocr_required = page_requires_ocr(&page_text.all());
            pages.push(ParsedPage {
                page_number: page_number as u32 + 1,
                width: page_width,
                height: page_height,
                ocr_required: page_ocr_required,
            });
            for segment in page_text.segments().iter() {
                let text = segment
                    .text()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if text.is_empty() {
                    continue;
                }
                text_bytes = text_bytes.saturating_add(text.len());
                if text_bytes > self.limits.max_text_bytes {
                    return Err(PdfParserError::Limit("text bytes"));
                }
                if blocks.len() >= self.limits.max_blocks {
                    return Err(PdfParserError::Limit("block count"));
                }
                let bounds = segment.bounds();
                let x = (bounds.left().value / page_width).clamp(0.0, 1.0);
                let y = ((page_height - bounds.top().value) / page_height).clamp(0.0, 1.0);
                let width = (bounds.width().value / page_width).clamp(0.0, 1.0 - x);
                let height = (bounds.height().value / page_height).clamp(0.0, 1.0 - y);
                let bbox = NormalizedBoundingBox::new(x, y, width, height).ok();
                blocks.push(ParsedBlock {
                    page_number: page_number as u32 + 1,
                    page_width,
                    page_height,
                    ordinal: blocks.len() as u32,
                    kind: "text".to_owned(),
                    text: text.clone(),
                    bbox,
                    content_hash: content_sha256(text.as_bytes()),
                });
            }
        }
        let ocr_required = pages.is_empty() || pages.iter().any(|page| page.ocr_required);
        Ok(ParsedDocument {
            pages,
            ocr_required,
            blocks,
        })
    }
}

fn page_requires_ocr(text: &str) -> bool {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .count()
        < 20
}

impl DocumentParser for PdfiumParser {
    fn version(&self) -> &'static str {
        crate::PARSER_VERSION
    }

    fn parse_file(&self, path: &Path) -> Result<ParsedDocument, PdfParserError> {
        PdfiumParser::parse_file(self, path)
    }
}

pub fn parse_pdf_file(path: &Path) -> Result<ParsedDocument, PdfParserError> {
    PdfiumParser::from_env()?.parse_file(path)
}

impl From<PdfiumError> for PdfParserError {
    fn from(error: PdfiumError) -> Self {
        Self::Document(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::page_requires_ocr;

    #[test]
    fn near_empty_pages_require_ocr() {
        assert!(page_requires_ocr("  1  "));
        assert!(!page_requires_ocr(
            "A sufficiently long extractable page of study text"
        ));
    }
}
