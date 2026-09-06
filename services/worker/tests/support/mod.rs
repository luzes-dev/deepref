#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
pub const DURABILITY_MIGRATION: &str =
    include_str!("../../../../crates/postgres/migrations/0004_ingestion_durability.sql");
