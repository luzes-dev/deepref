mod support;

#[test]
fn durability_schema_supports_idempotent_repair() {
    assert!(support::DURABILITY_MIGRATION.contains("work_event_id"));
    assert!(
        include_str!("../../../crates/postgres/migrations/0008_infrastructure_collapse.sql")
            .contains("jobs_expired_running_idx")
    );
}
