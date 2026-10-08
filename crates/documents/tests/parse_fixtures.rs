//! Regression checks on real papers. They run when `PDFIUM_LIBRARY_PATH` and
//! `DEEPREF_PDF_FIXTURES` (a directory holding the PDFs) are set, and pass without them.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use std::path::PathBuf;

use deepref_documents::{PdfiumParser, StructuredDocument};

/// The fixture whose file name starts with `prefix`, when the environment provides fixtures.
fn fixture(prefix: &str) -> Option<PathBuf> {
    std::env::var_os("PDFIUM_LIBRARY_PATH")?;
    let dir = PathBuf::from(std::env::var_os("DEEPREF_PDF_FIXTURES")?);
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().is_some_and(|ext| ext == "pdf")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
        })
}

fn parse(path: &std::path::Path) -> StructuredDocument {
    PdfiumParser::shared_from_env()
        .expect("pdfium")
        .parse_structured(path)
        .expect("fixture parses")
}

#[test]
fn comparison_signs_in_symbol_fonts_are_kept() {
    let Some(path) = fixture("ng-et-al-2007") else {
        return;
    };
    let parsed = parse(&path);
    let texts: Vec<&str> = parsed
        .document
        .blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect();
    let all = texts.join("\n");
    assert!(all.contains("P<0.001"), "less-than sign was lost");
    assert!(all.contains("P=0.535"), "equals sign was lost");
    assert!(all.contains("\u{2212}0.964"), "minus sign was lost");
    assert!(!all.contains("P0.001"), "a comparison sign was dropped");
}

#[test]
fn front_matter_and_captions_never_sit_under_a_heading() {
    let Some(path) = fixture("ng-et-al-2007") else {
        return;
    };
    let parsed = parse(&path);
    let mut checked = 0;
    for (block, structure) in parsed.document.blocks.iter().zip(&parsed.block_structure) {
        if matches!(structure.role.as_str(), "title" | "front_matter") {
            checked += 1;
            assert!(
                structure.section_path.is_empty(),
                "front matter {:?} has path {:?}",
                block.text,
                structure.section_path
            );
        }
        assert!(
            !(structure.role == "heading" && block.text.starts_with("TABLE ")),
            "a table caption became a heading: {:?}",
            block.text
        );
    }
    assert!(checked > 0, "the title or front matter was not found");
}

#[test]
fn a_two_line_results_heading_is_not_merged_with_its_subheading() {
    let Some(path) = fixture("ng-et-al-2007") else {
        return;
    };
    let parsed = parse(&path);
    for structure in &parsed.block_structure {
        assert!(
            !structure
                .section_path
                .iter()
                .any(|title| title.contains("Results General Demographics")),
            "merged heading in path {:?}",
            structure.section_path
        );
    }
}
