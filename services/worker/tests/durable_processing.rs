#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
mod support;

#[test]
fn claims_and_doi_leases_have_owner_expiry_and_attempts() {
    for field in [
        "owner_token",
        "lease_expires_at",
        "attempts",
        "completed_at",
        "heartbeat_at",
    ] {
        assert!(support::DURABILITY_MIGRATION.contains(field));
    }
}
