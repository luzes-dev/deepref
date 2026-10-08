//! Glue between the PDFium char stream and the pure stages.

use crate::chars::{RawChar, build_words};
use crate::layout::build_blocks;
use crate::structure::{PageBlocks, Structure, analyze_structure};

pub struct PageInput {
    pub page_number: u32,
    pub height: f32,
    pub chars: Vec<RawChar>,
}

/// chars -> words -> blocks (per page), then document-wide roles, sections and references.
pub fn analyze(pages: Vec<PageInput>) -> Structure {
    let pages = pages
        .into_iter()
        .map(|page| PageBlocks {
            page_number: page.page_number,
            height: page.height,
            blocks: build_blocks(&build_words(&page.chars)),
        })
        .collect();
    analyze_structure(pages)
}
