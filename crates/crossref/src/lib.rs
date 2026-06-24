mod client;
mod error;
mod status;
mod wire;

pub use client::CrossrefClient;
pub use error::CrossrefError;
pub use status::{StatusClassification, classify_status};
