#[test]
fn migrations_are_append_only_and_define_durability_contract() {
    let durability = include_str!("../../postgres/migrations/0004_ingestion_durability.sql");
    let projection = include_str!("../../postgres/migrations/0005_domain_projection.sql");
    let evidence = include_str!("../../postgres/migrations/0006_evidence_workspace.sql");
    let identity = include_str!("../../postgres/migrations/0007_evidence_identity.sql");
    let acquisition = include_str!("../../postgres/migrations/0009_acquisition_runs.sql");
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
    for required in [
        "ALTER TABLE citations RENAME TO legacy_citations",
        "source_report_id uuid",
        "target_report_id uuid",
        "CREATE TABLE acquisition_runs",
        "max_depth integer NOT NULL",
        "seed_count integer NOT NULL",
        "queued_count integer NOT NULL",
        "fetched_count integer NOT NULL",
        "failed_count integer NOT NULL",
        "CREATE TABLE record_provenance",
        "last_error text",
        "work_event_id uuid",
        "FROM legacy_citations",
    ] {
        assert!(
            identity.contains(required),
            "missing migration contract: {required}"
        );
    }
    for required in [
        "legacy_ingestion_id DROP NOT NULL",
        "acquisition_runs_project_idempotency_idx",
        "record_identifiers",
        "source_identifiers jsonb",
        "authors jsonb",
    ] {
        assert!(
            acquisition.contains(required),
            "missing acquisition migration contract: {required}"
        );
    }
}
