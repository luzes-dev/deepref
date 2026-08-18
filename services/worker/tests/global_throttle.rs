#[test]
fn provider_permit_schedule_is_postgresql_backed() {
    let migration = include_str!("../../api/migrations/0004_ingestion_durability.sql");
    let implementation = include_str!("../src/limiter.rs");
    assert!(migration.contains("provider_rate_state"));
    assert!(implementation.contains("FOR UPDATE"));
    assert!(!implementation.contains("governor"));
}
