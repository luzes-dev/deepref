use std::{path::Path, time::Instant};

use deepref_domain::NormalizedBoundingBox;
use pdfium_render::prelude::{Pdfium, PdfiumError};
use thiserror::Error;

use crate::chars::RawChar;
use crate::pipeline::{PageInput, analyze};
use crate::{
    BlockStructure, ParsedBlock, ParsedDocument, ParsedPage, ParsedReference, StructuredDocument,
    content_sha256,
};

pub trait DocumentParser: Send + Sync {
    fn version(&self) -> &'static str;
    fn parse_file(&self, path: &Path) -> Result<ParsedDocument, PdfParserError>;

    /// Like [`Self::parse_file`] plus roles, section paths and references. Parsers without
    /// structure support return the plain parse with empty section paths.
    fn parse_structured(&self, path: &Path) -> Result<StructuredDocument, PdfParserError> {
        self.parse_file(path).map(StructuredDocument::unstructured)
    }
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

    /// One parser per process. Dropping a `Pdfium` tears down the shared PDFium library, so
    /// per-job parsers corrupt the heap when two parses overlap in a multi-threaded worker.
    pub fn shared_from_env() -> Result<std::sync::Arc<Self>, PdfParserError> {
        // Initialization must happen once: a losing racer's `Pdfium` would be dropped and
        // tear the library down. A failed init is not cached, so a later call can retry.
        static SHARED: std::sync::Mutex<Option<std::sync::Arc<PdfiumParser>>> =
            std::sync::Mutex::new(None);
        let mut shared = SHARED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(parser) = shared.as_ref() {
            return Ok(parser.clone());
        }
        let parser = std::sync::Arc::new(Self::from_env()?);
        *shared = Some(parser.clone());
        Ok(parser)
    }

    /// Builds a parser that owns PDFium's lifetime; prefer [`Self::shared_from_env`] anywhere
    /// more than one parse can run in the process.
    pub fn from_env() -> Result<Self, PdfParserError> {
        let library_path = std::env::var_os("PDFIUM_LIBRARY_PATH");
        Self::from_config(
            library_path.as_deref().map(Path::new),
            ParserLimits::default(),
        )
    }

    pub fn parse_file(&self, path: &Path) -> Result<ParsedDocument, PdfParserError> {
        self.parse_structured(path)
            .map(|structured| structured.document)
    }

    pub fn parse_structured(&self, path: &Path) -> Result<StructuredDocument, PdfParserError> {
        // PDFium is not re-entrant across documents; serialize whole parses in this process.
        static PARSE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = PARSE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let started = Instant::now();
        let document = self
            .pdfium
            .load_pdf_from_file(path, None)
            .map_err(|error| PdfParserError::Document(error.to_string()))?;
        let page_count = document.pages().len();
        if page_count > self.limits.max_pages as i32 {
            return Err(PdfParserError::Limit("page count"));
        }
        let mut pages = Vec::with_capacity(page_count as usize);
        let mut inputs = Vec::with_capacity(page_count as usize);
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
            let all_text = page_text.all();
            text_bytes = text_bytes.saturating_add(all_text.len());
            if text_bytes > self.limits.max_text_bytes {
                return Err(PdfParserError::Limit("text bytes"));
            }
            pages.push(ParsedPage {
                page_number: page_number as u32 + 1,
                width: page_width,
                height: page_height,
                ocr_required: page_requires_ocr(&all_text),
            });
            let mut chars = Vec::new();
            for character in page_text.chars().iter() {
                let Some(ch) = character.unicode_char() else {
                    continue;
                };
                // Rotated text (margin stamps, watermarks) would bridge unrelated lines.
                if let Ok(angle) = character.angle_degrees() {
                    let angle = angle.rem_euclid(360.0);
                    if (5.0..=355.0).contains(&angle) {
                        continue;
                    }
                }
                let Ok(bounds) = character
                    .loose_bounds()
                    .or_else(|_| character.tight_bounds())
                else {
                    continue;
                };
                let size = character.scaled_font_size().value;
                if !size.is_finite() || size <= 0.0 {
                    continue;
                }
                let name = character.font_name().to_lowercase();
                let bold = character
                    .font_weight()
                    .is_some_and(|weight| is_bold_weight(&weight))
                    || ["bold", "black", "heavy", "semibold", "demi"]
                        .iter()
                        .any(|marker| name.contains(marker));
                let baseline = character
                    .origin_y()
                    .map_or(bounds.bottom().value, |y| y.value);
                chars.push(RawChar {
                    ch: math_symbol_glyph(ch, &name),
                    generated: character.is_generated().unwrap_or(false),
                    left: bounds.left().value,
                    right: bounds.right().value,
                    bottom: bounds.bottom().value,
                    top: bounds.top().value,
                    baseline,
                    size,
                    bold,
                });
            }
            inputs.push(PageInput {
                page_number: page_number as u32 + 1,
                height: page_height,
                chars,
            });
        }
        let structure = analyze(inputs);
        let mut blocks = Vec::new();
        let mut block_structure = Vec::new();
        for structured in structure.blocks {
            if blocks.len() >= self.limits.max_blocks {
                return Err(PdfParserError::Limit("block count"));
            }
            let Some(page) = pages.get(structured.page_index) else {
                continue;
            };
            let (page_width, page_height) = (page.width, page.height);
            let block = structured.block;
            let x = (block.left / page_width).clamp(0.0, 1.0);
            let y = ((page_height - block.top) / page_height).clamp(0.0, 1.0);
            let width = ((block.right - block.left) / page_width).clamp(0.0, 1.0 - x);
            let height = ((block.top - block.bottom) / page_height).clamp(0.0, 1.0 - y);
            let kind = structured.role.kind();
            blocks.push(ParsedBlock {
                page_number: page.page_number,
                page_width,
                page_height,
                ordinal: blocks.len() as u32,
                kind: kind.to_owned(),
                content_hash: content_sha256(block.text.as_bytes()),
                text: block.text,
                bbox: NormalizedBoundingBox::new(x, y, width, height).ok(),
            });
            block_structure.push(BlockStructure {
                role: kind.to_owned(),
                section_path: structured.section_path,
            });
        }
        let ocr_required = pages.is_empty() || pages.iter().any(|page| page.ocr_required);
        Ok(StructuredDocument {
            document: ParsedDocument {
                pages,
                ocr_required,
                blocks,
            },
            block_structure,
            references: structure
                .references
                .into_iter()
                .map(|reference| ParsedReference {
                    page_number: reference.page_number,
                    raw_text: reference.raw_text,
                    doi: reference.doi,
                    year: reference.year,
                })
                .collect(),
        })
    }
}

/// Math symbol fonts such as "Universal-GreekwithMathPi" carry no usable ToUnicode map, so PDFium
/// reports their `<`, `=` and minus glyphs as control characters, which the text pipeline drops.
/// Only these observed glyph codes are mapped back, and only inside those fonts: U+0002 is a
/// soft hyphen in ordinary text fonts.
fn math_symbol_glyph(code: char, font: &str) -> char {
    let font = font.to_lowercase();
    if font.contains("greekwithmathpi") {
        return match code {
            '\u{1}' => '<',
            '\u{2}' => '=',
            '\u{4}' => '\u{2212}',
            _ => code,
        };
    }
    if font.contains("mathematicalpi") && code == '\u{1}' {
        return '\u{2265}';
    }
    code
}

fn is_bold_weight(weight: &pdfium_render::prelude::PdfFontWeight) -> bool {
    use pdfium_render::prelude::PdfFontWeight as W;
    match weight {
        W::Weight600 | W::Weight700Bold | W::Weight800 | W::Weight900 => true,
        W::Custom(value) => *value >= 600,
        _ => false,
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

    fn parse_structured(&self, path: &Path) -> Result<StructuredDocument, PdfParserError> {
        PdfiumParser::parse_structured(self, path)
    }
}

pub fn parse_pdf_file(path: &Path) -> Result<ParsedDocument, PdfParserError> {
    PdfiumParser::shared_from_env()?.parse_file(path)
}

impl From<PdfiumError> for PdfParserError {
    fn from(error: PdfiumError) -> Self {
        Self::Document(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{PdfiumParser, math_symbol_glyph, page_requires_ocr};
    use std::path::Path;

    #[test]
    fn math_symbol_glyphs_are_mapped_only_inside_symbol_fonts() {
        assert_eq!(math_symbol_glyph('\u{1}', "Universal-GreekwithMathPi"), '<');
        assert_eq!(math_symbol_glyph('\u{2}', "Universal-GreekwithMathPi"), '=');
        assert_eq!(
            math_symbol_glyph('\u{4}', "Universal-GreekwithMathPi"),
            '\u{2212}'
        );
        assert_eq!(math_symbol_glyph('\u{1}', "MathematicalPi-One"), '\u{2265}');
        // A soft hyphen in a text font keeps its meaning.
        assert_eq!(math_symbol_glyph('\u{2}', "Times-Roman"), '\u{2}');
        assert_eq!(math_symbol_glyph('a', "Universal-GreekwithMathPi"), 'a');
    }

    /// Parses real PDFs when `PDFIUM_LIBRARY_PATH` and `DEEPREF_TEST_PDF` are set.
    #[test]
    fn real_pdf_yields_paragraph_blocks() {
        let (Some(_), Some(pdf)) = (
            std::env::var_os("PDFIUM_LIBRARY_PATH"),
            std::env::var_os("DEEPREF_TEST_PDF"),
        ) else {
            return;
        };
        let parsed = PdfiumParser::shared_from_env()
            .expect("pdfium")
            .parse_file(Path::new(&pdf))
            .expect("parse");
        let pages = parsed.pages.len();
        if std::env::var_os("DEEPREF_DUMP_BLOCKS").is_some() {
            for block in &parsed.blocks {
                println!("p{} {:?}", block.page_number, block.text);
            }
        }
        assert!(
            parsed.blocks.len() < pages * 40,
            "{} blocks for {pages} pages",
            parsed.blocks.len()
        );
    }

    /// Overlapping parses must not tear down PDFium under each other (a worker parses jobs on
    /// several blocking threads). Runs when `PDFIUM_LIBRARY_PATH` and `DEEPREF_TEST_PDF` are set.
    #[test]
    fn concurrent_parses_share_one_pdfium() {
        let (Some(_), Some(pdf)) = (
            std::env::var_os("PDFIUM_LIBRARY_PATH"),
            std::env::var_os("DEEPREF_TEST_PDF"),
        ) else {
            return;
        };
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let pdf = pdf.clone();
                std::thread::spawn(move || {
                    for _ in 0..3 {
                        let parser = PdfiumParser::shared_from_env().expect("pdfium");
                        let parsed = parser.parse_file(Path::new(&pdf)).expect("parse");
                        assert!(!parsed.blocks.is_empty());
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("parse thread");
        }
    }

    fn pages_len(parsed: &crate::StructuredDocument) -> usize {
        parsed.document.pages.len().max(1)
    }

    pub(crate) fn metrics_for(texts: &[String]) -> (usize, usize) {
        let mut tokens = 0usize;
        let mut glued = 0usize;
        for text in texts {
            for token in text.split_whitespace() {
                tokens += 1;
                if token
                    .chars()
                    .any(|c| !c.is_alphabetic() && c != '-' && c != ',' && c != '.')
                {
                    continue; // URLs, identifiers and primer names legitimately mix case
                }
                let chars: Vec<char> = token.chars().collect();
                if chars.len() > 20
                    && chars
                        .windows(2)
                        .any(|pair| pair[0].is_lowercase() && pair[1].is_uppercase())
                {
                    glued += 1;
                }
            }
        }
        (tokens, glued)
    }

    /// Prints quality metrics for every PDF in `DEEPREF_PDF_FIXTURES` (a directory). Runs when
    /// `PDFIUM_LIBRARY_PATH` and `DEEPREF_PDF_FIXTURES` are set; always passes unless parsing
    /// fails: run with `--nocapture` and read the numbers.
    #[test]
    fn fixture_metrics() {
        let (Some(_), Some(dir)) = (
            std::env::var_os("PDFIUM_LIBRARY_PATH"),
            std::env::var_os("DEEPREF_PDF_FIXTURES"),
        ) else {
            return;
        };
        let parser = PdfiumParser::shared_from_env().expect("pdfium");
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .expect("fixture dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "pdf"))
            .collect();
        entries.sort();
        for path in entries {
            let started = std::time::Instant::now();
            let Ok(parsed) = parser.parse_structured(&path) else {
                println!("{}: parse failed", path.display());
                continue;
            };
            if std::env::var_os("DEEPREF_METRICS_BASELINE").is_some() {
                // Pre-rewrite behaviour: one block per PDFium text segment.
                let document = parser.pdfium.load_pdf_from_file(&path, None).expect("open");
                let mut segments = Vec::new();
                for page in document.pages().iter() {
                    if let Ok(text) = page.text() {
                        segments.extend(text.segments().iter().map(|s| s.text()));
                    }
                }
                let (tokens, glued) = metrics_for(&segments);
                println!(
                    "  baseline(segments): blocks/page={:.1} glued={glued}/{tokens} ({:.2}/1k tokens)",
                    segments.len() as f32 / pages_len(&parsed) as f32,
                    1000.0 * glued as f32 / tokens.max(1) as f32,
                );
            }
            let texts: Vec<String> = parsed
                .document
                .blocks
                .iter()
                .map(|block| block.text.clone())
                .collect();
            let (tokens, glued) = metrics_for(&texts);
            let pages = parsed.document.pages.len().max(1);
            let body: Vec<_> = parsed
                .block_structure
                .iter()
                .filter(|s| s.role == "text")
                .collect();
            let with_path = body.iter().filter(|s| !s.section_path.is_empty()).count();
            println!(
                "{}: pages={pages} blocks/page={:.1} glued={glued}/{tokens} ({:.2}/1k tokens) sectioned_body={}/{} refs={} refs_doi={} ms={}",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                parsed.document.blocks.len() as f32 / pages as f32,
                1000.0 * glued as f32 / tokens.max(1) as f32,
                with_path,
                body.len(),
                parsed.references.len(),
                parsed.references.iter().filter(|r| r.doi.is_some()).count(),
                started.elapsed().as_millis(),
            );
            if std::env::var_os("DEEPREF_DUMP_BLOCKS").is_some() {
                for (block, structure) in parsed.document.blocks.iter().zip(&parsed.block_structure)
                {
                    println!(
                        "p{} [{}] {:?} :: {:?}",
                        block.page_number, block.kind, structure.section_path, block.text
                    );
                }
            }
        }
    }

    #[test]
    fn near_empty_pages_require_ocr() {
        assert!(page_requires_ocr("  1  "));
        assert!(!page_requires_ocr(
            "A sufficiently long extractable page of study text"
        ));
    }
}
