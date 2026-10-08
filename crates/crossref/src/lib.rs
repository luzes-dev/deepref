mod abstracts;
mod client;
mod error;
mod status;
mod unpaywall;
mod wire;

pub use abstracts::{AbstractEnricher, AbstractSource, clean_abstract};
pub use client::CrossrefClient;
pub use error::CrossrefError;
pub use status::{StatusClassification, classify_status};
pub use unpaywall::{UnpaywallClient, UnpaywallError, pdf_url_from_response};
