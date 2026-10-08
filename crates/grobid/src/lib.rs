//! Client and TEI parser for GROBID (https://github.com/kermitt2/grobid).
//!
//! GROBID turns a PDF into TEI XML with a section tree and a structured
//! bibliography. This crate only talks HTTP and parses TEI; merging the result
//! into DeepRef's own parsed blocks lives in the worker.

mod client;
mod tei;

pub use client::{GrobidClient, GrobidConfig, GrobidError};
pub use tei::{TeiDocument, TeiError, TeiReference, TeiSection, parse_tei};
