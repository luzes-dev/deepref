mod support;

#[test]
fn durability_schema_supports_idempotent_repair() {
    assert!(support::DURABILITY_MIGRATION.contains("work_event_id"));
    assert!(support::DURABILITY_MIGRATION.contains("event_outbox_retry_idx"));
}
