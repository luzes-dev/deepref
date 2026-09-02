#[test]
fn provider_permit_schedule_is_postgresql_backed() {
    let migration =
        include_str!("../../../crates/postgres/migrations/0004_ingestion_durability.sql");
    let persistence = include_str!("../../../crates/postgres/src/worker_runtime.rs");
    let worker = include_str!("../src/limiter.rs");

    assert!(migration.contains("provider_rate_state"));
    assert!(persistence.contains("provider_rate_state"));
    assert!(persistence.contains("FOR UPDATE"));
    assert!(!worker.contains("provider_rate_state"));
    assert!(!worker.contains("FOR UPDATE"));
    assert!(!worker.contains("governor"));
}
