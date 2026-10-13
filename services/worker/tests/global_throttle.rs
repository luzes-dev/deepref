#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
#[test]
fn provider_permit_schedule_is_postgresql_backed() {
    let migration =
        include_str!("../../../crates/app/migrations/0004_ingestion_durability.sql");
    let persistence = include_str!("../../../crates/app/src/worker_runtime.rs");
    let worker = include_str!("../src/limiter.rs");

    assert!(migration.contains("provider_rate_state"));
    assert!(persistence.contains("provider_rate_state"));
    assert!(persistence.contains("FOR UPDATE"));
    assert!(!worker.contains("provider_rate_state"));
    assert!(!worker.contains("FOR UPDATE"));
    assert!(!worker.contains("governor"));
}
