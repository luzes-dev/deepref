#[test]
fn migrations_are_append_only_and_define_durability_contract() {
    let durability = include_str!("../../postgres/migrations/0004_ingestion_durability.sql");
    let projection = include_str!("../../postgres/migrations/0005_domain_projection.sql");
    let evidence = include_str!("../../postgres/migrations/0006_evidence_workspace.sql");
    for required in [
        "owner_token",
        "lease_expires_at",
        "dead_letter_records",
        "provider_rate_state",
    ] {
        assert!(durability.contains(required));
    }
    for required in [
        "graph_domain_revision_seq",
        "domain_events",
        "projection_state",
        "metric_snapshots",
    ] {
        assert!(projection.contains(required));
    }
    for required in [
        "reports",
        "report_identifiers",
        "screening_events",
        "screening_state",
        "protocol_versions",
        "jobs",
        "prisma_snapshots",
    ] {
        assert!(evidence.contains(required));
    }
}
