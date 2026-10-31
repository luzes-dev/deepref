#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
mod support;

#[test]
fn durability_schema_supports_idempotent_repair() {
    assert!(support::BASELINE_SCHEMA.contains("work_event_id"));
    assert!(support::BASELINE_SCHEMA.contains("jobs_expired_running_idx"));
    assert!(support::BASELINE_SCHEMA.contains("record_identifiers"));
}
