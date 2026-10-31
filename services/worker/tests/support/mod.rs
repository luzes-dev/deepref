#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
pub const BASELINE_SCHEMA: &str =
    include_str!("../../../../crates/app/migrations/0001_baseline.sql");
