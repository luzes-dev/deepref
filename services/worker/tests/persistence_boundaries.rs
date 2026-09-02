#[test]
fn reconciliation_schema_knowledge_stays_in_postgres_adapter() {
    let persistence = include_str!("../../../crates/postgres/src/worker_runtime.rs");
    let worker = include_str!("../src/reconciler.rs");

    for table in [
        "processed_events",
        "doi_fetch_state",
        "ingestion_items",
        "ingestions",
        "jobs",
    ] {
        assert!(
            persistence.contains(table),
            "postgres worker runtime must own {table} persistence"
        );
        assert!(
            !worker.contains(table),
            "worker reconciler must not encode {table} schema knowledge"
        );
    }
    assert!(!worker.contains("sqlx::query"));
}
