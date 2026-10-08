//! Live check of the GROBID enrichment path. Ignored by default; run with
//! `GROBID_URL=http://127.0.0.1:8070 GROBID_TEST_PDF=/path/to.pdf PDFIUM_LIBRARY_PATH=... \
//!  cargo test -p deepref-worker --test grobid_enrichment -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use deepref_grobid::{GrobidClient, GrobidConfig};
use deepref_worker::enrichment::{BlockView, build_structure};

#[tokio::test]
#[ignore = "needs a running GROBID, PDFium and a sample PDF"]
async fn enriches_a_real_pdf() {
    let url = std::env::var("GROBID_URL").expect("GROBID_URL");
    let path = std::env::var("GROBID_TEST_PDF").expect("GROBID_TEST_PDF");
    let bytes = std::fs::read(&path).unwrap();
    let parsed = deepref_documents::parse_pdf_file(std::path::Path::new(&path)).unwrap();
    let client = GrobidClient::new(GrobidConfig::new(url)).unwrap();
    assert!(client.is_alive().await);
    let tei = client.process_fulltext(&bytes).await.unwrap();
    let empty: Vec<String> = Vec::new();
    let views: Vec<BlockView<'_>> = parsed
        .blocks
        .iter()
        .map(|b| BlockView {
            ordinal: b.ordinal,
            text: &b.text,
            native_role: "text",
            native_path: &empty,
        })
        .collect();
    let enrichment = build_structure(&views, &tei);
    println!(
        "title={:?}\nblocks={} grobid_sections={} references={} labelled_blocks={} suffix={}",
        tei.title,
        parsed.blocks.len(),
        tei.sections.len(),
        tei.references.len(),
        enrichment.structure.section_paths.len(),
        enrichment.parser_suffix
    );
    for section in tei.sections.iter().take(6) {
        println!("  section {:?} {:?}", section.number, section.path);
    }
    for reference in tei.references.iter().take(3) {
        println!(
            "  ref {:?} / {:?} / {:?}",
            reference.authors, reference.title, reference.year
        );
    }
    assert!(!tei.sections.is_empty());
    assert!(!tei.references.is_empty());
    assert!(!enrichment.structure.section_paths.is_empty());
}
