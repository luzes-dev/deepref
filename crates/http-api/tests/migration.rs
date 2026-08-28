#[test]
fn migrations_are_append_only_and_define_durability_contract() {
    let durability = include_str!("../../postgres/migrations/0004_ingestion_durability.sql");
    let projection = include_str!("../../postgres/migrations/0005_domain_projection.sql");
    let evidence = include_str!("../../postgres/migrations/0006_evidence_workspace.sql");
    let identity = include_str!("../../postgres/migrations/0007_evidence_identity.sql");
    let acquisition = include_str!("../../postgres/migrations/0009_acquisition_runs.sql");
    let deduplication = include_str!("../../postgres/migrations/0010_deduplication.sql");
    let studies_appraisals = include_str!("../../postgres/migrations/0014_studies_appraisals.sql");
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
    for required in [
        "dedupe_proposals",
        "dedupe_resolution_events",
        "reports_normalized_title_trgm_idx",
        "Rust NFKC",
        "dedupe_proposals_candidate_project_report_fkey",
        "dedupe_resolution_events_prior_project_report_fkey",
        "dedupe_resolution_events_resolved_project_report_fkey",
        "dedupe_resolution_events_proposal_project_fkey",
        "dedupe_proposals_status_reviewer_check",
        "reverted_event_id",
    ] {
        assert!(
            deduplication.contains(required),
            "missing deduplication migration contract: {required}"
        );
    }
    for required in [
        "study_revision",
        "studies_title_check",
        "studies_design_check",
        "study_reports_study_project_fk",
        "study_reports_report_project_fk",
        "study_events",
        "appraisal_assessments",
        "appraisal_assessment_evidence",
        "appraisal_events",
        "definition_version",
        "ON DELETE CASCADE",
    ] {
        assert!(
            studies_appraisals.contains(required),
            "missing PR9 migration contract: {required}"
        );
    }
    assert!(!studies_appraisals.contains("normalized_design"));
}
