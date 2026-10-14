#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use deepref_app::MIGRATOR;
use sqlx::{PgPool, migrate::Migrator};
use std::time::Duration;
use uuid::Uuid;

const PROJECT: &str = "00000000-0000-0000-0000-000000000001";

async fn assert_database(pool: &PgPool, sql: &'static str, message: &str) {
    let matches: bool = sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .expect(message);
    assert!(matches, "{message}");
}

#[tokio::test]
async fn populated_legacy_database_expands_backfills_and_retires_doi_plane() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = PgPool::connect(&url)
        .await
        .expect("private empty database connects");
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < 70)
            .cloned()
            .collect(),
    )
    .run(&pool)
    .await
    .expect("only pre-0070 migrations apply before historical fixtures");
    sqlx::raw_sql(SEED)
        .execute(&pool)
        .await
        .expect("populated legacy fixture inserts");
    MIGRATOR
        .run(&pool)
        .await
        .expect("0070 through full DOI retirement migrate populated data");

    assert_database(
        &pool,
        "SELECT default_max_depth=4 AND historical_default_max_depth=4 FROM projects",
        "historical project default survives without silently clamping",
    )
    .await;
    assert_database(
        &pool,
        "SELECT default_max_depth=5 AND historical_default_max_depth=5 FROM settings WHERE id=1",
        "historical operator default survives without blocking cutover",
    )
    .await;
    assert_database(&pool, "SELECT NOT EXISTS (SELECT 1 FROM fixture_items f FULL JOIN acquisition_items a ON a.acquisition_id=f.ingestion_id AND a.canonical_doi=f.canonical_doi WHERE (to_jsonb(a)-'report_id'-'acquisition_id') IS DISTINCT FROM (to_jsonb(f)-'ingestion_id'))", "every item preserves depth, parent, attempts, errors, timestamps and job ID").await;
    assert_database(&pool, "SELECT NOT EXISTS (SELECT 1 FROM fixture_runs f JOIN acquisition_runs a ON a.id=f.id WHERE (to_jsonb(a)->'max_depth') IS DISTINCT FROM (to_jsonb(f)->'max_depth') OR a.historical_max_depth<>f.max_depth OR a.seed_count<>f.seed_count OR a.created_at<>f.created_at OR a.started_at IS DISTINCT FROM f.started_at OR a.completed_at IS DISTINCT FROM f.completed_at OR a.metadata_provider<>f.metadata_provider OR a.citation_provider<>f.citation_provider) AND (SELECT count(*) FROM acquisition_runs)=4", "all historical runs and their provenance survive").await;
    assert_database(&pool, "SELECT bool_and((status,seed_count,queued_count,fetched_count,failed_count) = CASE id WHEN '00000000-0000-0000-0000-000000000011'::uuid THEN ROW('completed'::text,7,0,1,0) WHEN '00000000-0000-0000-0000-000000000012'::uuid THEN ROW('failed'::text,11,0,0,1) WHEN '00000000-0000-0000-0000-000000000013'::uuid THEN ROW('cancelled'::text,1,0,0,0) ELSE ROW('running'::text,2,2,0,0) END) FROM acquisition_runs", "counters derive from items without reviving cancelled runs").await;
    assert_database(&pool, "SELECT count(*)=2 AND bool_and(report_id='00000000-0000-0000-0000-000000000021'::uuid) FROM report_identifiers WHERE normalized_value IN ('10.5555/one','10.5555/alias')", "existing UUID and multiple DOI aliases remain canonical").await;
    assert_database(&pool, r#"SELECT title='Existing reviewed title' AND raw='{"reviewed":true}'::jsonb FROM reports WHERE id='00000000-0000-0000-0000-000000000021'"#, "existing report metadata is not replaced by cache metadata").await;
    assert_database(&pool, "SELECT NOT EXISTS(SELECT 1 FROM acquisition_items i LEFT JOIN project_reports p ON p.project_id=i.project_id AND p.report_id=i.report_id WHERE p.report_id IS NULL) AND (SELECT count(*) FROM project_reports)=3", "item-only reports acquire project membership and aliases do not duplicate memberships").await;
    assert_database(&pool, "SELECT count(*)=4 AND bool_and(r.acquisition_run_id=i.acquisition_id AND (r.raw->>'depth')::int=i.depth AND r.raw->>'parent_doi' IS NOT DISTINCT FROM i.parent_doi) FROM acquisition_items i JOIN records r ON r.source='doi_acquisition_item' AND r.source_key=i.acquisition_id::text||':'||i.canonical_doi", "each DOI observation has exact acquisition record provenance").await;
    assert_database(&pool, "SELECT count(*)=2 AND bool_and((raw->>'seed')::boolean=(source_key='10.5555/one') AND (raw->>'min_depth')::integer=CASE source_key WHEN '10.5555/one' THEN 0 ELSE 1 END) FROM records WHERE source='doi'", "legacy memberships retain seed and minimum depth").await;
    assert_database(&pool, "SELECT count(*)=1 AND bool_and(source_report_id='00000000-0000-0000-0000-000000000021'::uuid AND target_report_id=(SELECT report_id FROM report_identifiers WHERE normalized_value='10.5555/two') AND source='fixture-citation' AND acquisition_run_id='00000000-0000-0000-0000-000000000011'::uuid AND created_at='2020-01-02T03:04:05Z') FROM citations", "UUID citation preserves source, acquisition and creation time").await;
    assert_database(&pool, "SELECT NOT EXISTS(SELECT 1 FROM fixture_unresolved f FULL JOIN project_unresolved_references u ON u.project_id=f.project_id AND u.id=f.id WHERE (to_jsonb(u)-'source_report_id') IS DISTINCT FROM (to_jsonb(f)-'source_doi') OR u.source_report_id IS DISTINCT FROM '00000000-0000-0000-0000-000000000021'::uuid)", "unresolved identity, bibliographic fields and timestamp survive exactly").await;
    assert_database(&pool, "SELECT NOT EXISTS(SELECT 1 FROM fixture_jobs f LEFT JOIN jobs j ON j.id=f.id WHERE j.id IS NULL OR (to_jsonb(j)-'payload') IS DISTINCT FROM (to_jsonb(f)-'payload') OR j.payload->>'event_id' IS DISTINCT FROM j.id::text OR j.payload#>>'{payload,acquisition_id}' IS DISTINCT FROM COALESCE(f.payload#>>'{payload,ingestion_id}',f.payload->>'ingestion_id') OR j.payload#>>'{payload,doi}' IS DISTINCT FROM COALESCE(f.payload#>>'{payload,doi}',f.payload->>'doi'))", "queued, leased and dead jobs preserve IDs, dedupe, attempts and ownership while payload identity changes").await;
    assert_database(&pool, "SELECT status='fetched' AND provider='crossref' AND fetched_at='2020-01-02T03:04:05Z' FROM report_fetch_state WHERE report_id=(SELECT report_id FROM report_identifiers WHERE normalized_value='10.5555/two')", "global successful cache remains fetched despite failed run item").await;
    assert_database(&pool, "SELECT NOT EXISTS(SELECT 1 FROM fixture_fetch f JOIN report_identifiers i ON i.normalized_value=f.canonical_doi JOIN report_fetch_state s ON s.report_id=i.report_id WHERE (to_jsonb(s)-'report_id'-'provider') IS DISTINCT FROM to_jsonb(f))", "provider fetching lease, owner, attempts and timestamps survive").await;
    assert_database(&pool, "SELECT (SELECT jsonb_agg(to_jsonb(w) ORDER BY canonical_doi) FROM works w)=(SELECT jsonb_agg(to_jsonb(w)-'fetch_status' ORDER BY canonical_doi) FROM fixture_works w) AND (SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM fetched_unresolved_reference_facts f)=(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM fixture_facts f) AND (SELECT count(*) FROM fetched_citation_facts)=1 AND (SELECT count(*) FROM crossref_raw_records)=1", "provider cache and reusable fetched facts remain intact").await;
    assert_database(
        &pool,
        "SELECT metric_requested_revision=90 FROM projects",
        "requested revision floors events, snapshots and completed watermark",
    )
    .await;
    sqlx::query("SELECT recompute_project_report_metrics($1)")
        .bind(Uuid::parse_str(PROJECT).unwrap())
        .execute(&pool)
        .await
        .expect("v2 metrics remain callable after event retirement");
    assert_database(
        &pool,
        "SELECT revision=90 AND watermark=90 FROM projection_state WHERE project_id IS NOT NULL",
        "recomputation cannot regress completed revision floor",
    )
    .await;
    assert_database(
        &pool,
        "SELECT work_count=3 AND edge_count=1 FROM metric_snapshots WHERE revision=90",
        "v2 graph metric counts retain memberships and edge",
    )
    .await;
    sqlx::query("UPDATE projects SET metric_requested_revision=metric_requested_revision+1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("SELECT recompute_project_report_metrics($1)")
        .bind(Uuid::parse_str(PROJECT).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_database(
        &pool,
        "SELECT revision=91 AND watermark=91 FROM projection_state WHERE project_id IS NOT NULL",
        "new graph revision advances monotonically",
    )
    .await;

    let rejected = sqlx::query("INSERT INTO acquisition_runs(id,project_id,source,strategy,max_depth,created_at) VALUES ($1,$2,'crossref','citation_traversal',10,now())").bind(Uuid::new_v4()).bind(Uuid::parse_str(PROJECT).unwrap()).execute(&pool).await.expect_err("new depth ten must fail without historical provenance");
    assert_eq!(
        rejected
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    assert_database(&pool, "SELECT NOT EXISTS(SELECT 1 FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relname=ANY(ARRAY['ingestions','ingestion_items','project_works','legacy_citations','doi_fetch_state','unresolved_references','processed_events','dead_letter_records','domain_events','domain_tombstones','record_provenance','graph_domain_revision_seq'])) AND to_regprocedure('recompute_project_metrics(uuid)') IS NULL AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND ((table_name='works' AND column_name='fetch_status') OR (table_name='acquisition_runs' AND column_name='legacy_ingestion_id') OR (table_name='citations' AND column_name IN ('legacy_source_doi','legacy_target_doi','first_seen_ingestion_id'))))", "retired tables, sequence, function and compatibility columns are absent").await;
    let job = deepref_app::claim_job(&pool, "cutover-test", Duration::from_secs(60))
        .await
        .unwrap()
        .expect("pending migrated job is executable");
    assert_eq!(
        job.id,
        Uuid::parse_str("00000000-0000-0000-0000-000000000031").unwrap()
    );
    assert_eq!(
        job.payload["payload"]["acquisition_id"],
        "00000000-0000-0000-0000-000000000014"
    );
    let event: deepref_domain::events::EventEnvelope<deepref_domain::events::WorkFetchRequested> =
        serde_json::from_value(job.payload.clone())
            .expect("migrated pending payload executes without a compatibility parser");
    assert_eq!(event.event_id, job.id);
    assert_eq!(event.payload.max_depth, 5);
    assert!(
        deepref_app::claim_job(&pool, "second-worker", Duration::from_secs(60))
            .await
            .unwrap()
            .is_none(),
        "leased pending job executes only once"
    );
    MIGRATOR
        .run(&pool)
        .await
        .expect("SQLx migration replay is idempotent");
}

const SEED: &str = r#"
INSERT INTO projects(id,name,default_max_depth) VALUES ('00000000-0000-0000-0000-000000000001','cutover',4);
INSERT INTO settings(id,default_max_depth) VALUES (1,5) ON CONFLICT(id) DO UPDATE SET default_max_depth=EXCLUDED.default_max_depth;
INSERT INTO reports(id,title,raw) VALUES ('00000000-0000-0000-0000-000000000021','Existing reviewed title','{"reviewed":true}');
INSERT INTO report_identifiers(id,report_id,scheme,value,normalized_value) VALUES
('00000000-0000-0000-0000-000000000022','00000000-0000-0000-0000-000000000021','doi','10.5555/one','10.5555/one'),
('00000000-0000-0000-0000-000000000023','00000000-0000-0000-0000-000000000021','doi','10.5555/alias','10.5555/alias');
INSERT INTO works(canonical_doi,title,abstract_text,issued_year,published_year,container_title,url,work_type,publisher,total_citations,references_count,fetch_status,fetched_at,raw) VALUES
('10.5555/one','Cache one','Abstract one',2024,2025,'Journal one','https://example.test/one','article','Publisher one',27,4,'fetched','2020-01-02T03:04:05Z','{"fixture":1}'),
('10.5555/two','Cache two','Abstract two',2023,NULL,'Journal two','https://example.test/two','review','Publisher two',11,2,'fetched','2020-01-02T03:04:05Z','{"fixture":2}');
INSERT INTO ingestions(id,project_id,status,max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at,started_at,completed_at) SELECT id::uuid,'00000000-0000-0000-0000-000000000001',status,depth,seeds,8,9,10,'crossref','crossref','2020-01-01Z','2020-01-02Z','2020-01-03Z' FROM (VALUES
('00000000-0000-0000-0000-000000000011','completed',4,7),('00000000-0000-0000-0000-000000000012','completed',5,11),('00000000-0000-0000-0000-000000000013','cancelled',4,1),('00000000-0000-0000-0000-000000000014','running',5,2)) f(id,status,depth,seeds);
INSERT INTO project_works(project_id,canonical_doi,first_seen_ingestion_id,seed,min_depth) VALUES
('00000000-0000-0000-0000-000000000001','10.5555/one','00000000-0000-0000-0000-000000000011',true,0),
('00000000-0000-0000-0000-000000000001','10.5555/two','00000000-0000-0000-0000-000000000012',false,1);
INSERT INTO ingestion_items(ingestion_id,project_id,canonical_doi,depth,parent_doi,status,attempts,last_error,queued_at,fetched_at,work_event_id) SELECT run::uuid,'00000000-0000-0000-0000-000000000001',doi,depth,parent,status,attempts,error,'2020-01-01Z',fetched::timestamptz,event::uuid FROM (VALUES
('00000000-0000-0000-0000-000000000011','10.5555/one',1,'10.5555/parent','fetched',3,NULL,'2020-01-02T03:04:05Z','00000000-0000-0000-0000-000000000033'),
('00000000-0000-0000-0000-000000000012','10.5555/two',2,NULL,'failed',4,'legacy failure',NULL,'00000000-0000-0000-0000-000000000034'),
('00000000-0000-0000-0000-000000000014','10.5555/alias',4,'10.5555/one','queued',1,NULL,NULL,'00000000-0000-0000-0000-000000000031'),
('00000000-0000-0000-0000-000000000014','10.5555/item-only',5,'10.5555/alias','fetching',2,'previous retry',NULL,'00000000-0000-0000-0000-000000000032')) f(run,doi,depth,parent,status,attempts,error,fetched,event);
INSERT INTO legacy_citations(project_id,source_doi,target_doi,source,first_seen_ingestion_id,created_at) VALUES ('00000000-0000-0000-0000-000000000001','10.5555/one','10.5555/two','fixture-citation','00000000-0000-0000-0000-000000000011','2020-01-02T03:04:05Z');
INSERT INTO unresolved_references VALUES ('unresolved-exact','00000000-0000-0000-0000-000000000001','10.5555/one','Raw unstructured','Article title','Author','1998','12','34','2020-01-02T03:04:05Z');
INSERT INTO fetched_unresolved_reference_facts SELECT id,source_doi,raw_unstructured,article_title,author,year,volume,first_page,created_at FROM unresolved_references;
INSERT INTO fetched_citation_facts VALUES ('10.5555/one','10.5555/two','crossref-reference','2020-01-02T03:04:05Z');
INSERT INTO crossref_raw_records VALUES ('10.5555/one','{"provider":"crossref"}','2020-01-02T03:04:05Z');
INSERT INTO doi_fetch_state(canonical_doi,status,locked_at,fetched_at,last_error,updated_at,owner_token,lease_expires_at,heartbeat_at,attempts) VALUES ('10.5555/item-only','fetching','2020-01-01Z',NULL,'previous retry','2020-01-02Z','00000000-0000-0000-0000-000000000041','2099-01-01Z','2020-01-02Z',2);
INSERT INTO jobs(id,project_id,kind,payload,state,attempts,max_attempts,dedupe_key,last_error,lease_owner,leased_until,lease_renewed_at) VALUES
('00000000-0000-0000-0000-000000000031','00000000-0000-0000-0000-000000000001','work_fetch_requested','{"payload":{"ingestion_id":"00000000-0000-0000-0000-000000000014","doi":"10.5555/alias","depth":4}}','queued',1,5,'fixture-queued',NULL,NULL,NULL,NULL),
('00000000-0000-0000-0000-000000000032','00000000-0000-0000-0000-000000000001','work_fetch_requested','{"ingestion_id":"00000000-0000-0000-0000-000000000014","doi":"10.5555/item-only","depth":5}','running',2,5,'fixture-running','previous retry','old-worker','2099-01-01Z','2020-01-02Z'),
('00000000-0000-0000-0000-000000000034','00000000-0000-0000-0000-000000000001','work_fetch_requested','{"ingestion_id":"00000000-0000-0000-0000-000000000012","doi":"10.5555/two"}','dead',4,4,'fixture-dead','legacy failure',NULL,NULL,NULL);
INSERT INTO processed_events(event_id,processed_at,owner_token,lease_expires_at,attempts,completed_at,last_error) SELECT id,NULL,'00000000-0000-0000-0000-000000000041',leased_until,attempts,CASE WHEN state='dead' THEN now() END,last_error FROM jobs;
INSERT INTO dead_letter_records(identity,source_subject,source_event_id,delivery_count,reason_code,payload_sha256,payload,job_event_id) VALUES ('fixture-dlq','fetch_work','00000000-0000-0000-0000-000000000034',4,'exhausted','hash','{"reason":"legacy failure"}','00000000-0000-0000-0000-000000000034');
INSERT INTO domain_events(event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id) VALUES ('00000000-0000-0000-0000-000000000051',1,'metric','metric','00000000-0000-0000-0000-000000000001',70,'{}','00000000-0000-0000-0000-000000000001');
INSERT INTO domain_tombstones VALUES ('metric','fixture','00000000-0000-0000-0000-000000000001',70,'00000000-0000-0000-0000-000000000051',now());
INSERT INTO metric_snapshots VALUES ('00000000-0000-0000-0000-000000000001',80,now(),2,1,'{}');
INSERT INTO projection_state(projection_name,project_id,state,revision,watermark) VALUES ('postgres_graph','00000000-0000-0000-0000-000000000001','ready',85,90);
CREATE TABLE fixture_items AS TABLE ingestion_items;
CREATE TABLE fixture_runs AS TABLE ingestions;
CREATE TABLE fixture_unresolved AS TABLE unresolved_references;
CREATE TABLE fixture_jobs AS TABLE jobs;
CREATE TABLE fixture_fetch AS TABLE doi_fetch_state;
CREATE TABLE fixture_works AS TABLE works;
CREATE TABLE fixture_facts AS TABLE fetched_unresolved_reference_facts;
"#;
