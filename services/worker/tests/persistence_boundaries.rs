#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
fn contains_identifier(source: &str, identifier: &str) -> bool {
    source
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .any(|token| token == identifier)
}

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
            contains_identifier(persistence, table),
            "postgres worker runtime must own {table} persistence"
        );
        assert!(
            !contains_identifier(worker, table),
            "worker reconciler must not encode {table} schema knowledge"
        );
    }
    assert!(!worker.contains("sqlx::query"));
}
