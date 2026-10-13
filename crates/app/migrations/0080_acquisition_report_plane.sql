-- Offline expand/backfill: deployment must quiesce old producers and workers.
-- Existing identifier mappings, not deterministic UUID guesses, own report identity.
LOCK TABLE ingestions, ingestion_items, doi_fetch_state, works, project_works,
  legacy_citations, unresolved_references, processed_events, jobs IN ACCESS EXCLUSIVE MODE;

CREATE TEMP TABLE doi_report_map ON COMMIT DROP AS
SELECT doi, COALESCE(ri.report_id, md5('deepref:report:' || doi)::uuid) AS report_id
FROM (
 SELECT lower(canonical_doi) doi FROM works
 UNION SELECT lower(canonical_doi) FROM ingestion_items
 UNION SELECT lower(canonical_doi) FROM doi_fetch_state
 UNION SELECT lower(source_doi) FROM unresolved_references
) source
LEFT JOIN report_identifiers ri ON ri.scheme='doi' AND ri.normalized_value=source.doi;
CREATE UNIQUE INDEX ON doi_report_map(doi);

INSERT INTO reports(id,title,abstract_text,publication_year,journal,url,raw,
 work_type,publisher,container_title,total_citations,references_count)
SELECT m.report_id,w.title,w.abstract_text,COALESCE(w.published_year,w.issued_year),
 w.container_title,w.url,COALESCE(w.raw,'{}'::jsonb),w.work_type,w.publisher,w.container_title,
 COALESCE(w.total_citations,0),COALESCE(w.references_count,0)
FROM doi_report_map m LEFT JOIN works w ON lower(w.canonical_doi)=m.doi
ON CONFLICT(id) DO NOTHING;
INSERT INTO report_identifiers(id,report_id,scheme,value,normalized_value)
SELECT gen_random_uuid(),report_id,'doi',doi,doi FROM doi_report_map
ON CONFLICT(scheme,normalized_value) DO NOTHING;

INSERT INTO acquisition_runs(id,project_id,source,strategy,status,max_depth,historical_max_depth,
 seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,
 created_at,started_at,completed_at)
SELECT id,project_id,'crossref','citation_traversal',status,max_depth,historical_max_depth,
 seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,
 created_at,started_at,completed_at FROM ingestions
ON CONFLICT(id) DO UPDATE SET max_depth=EXCLUDED.max_depth,
 historical_max_depth=EXCLUDED.historical_max_depth,seed_count=EXCLUDED.seed_count,
 status=EXCLUDED.status,metadata_provider=EXCLUDED.metadata_provider,
 citation_provider=EXCLUDED.citation_provider,started_at=EXCLUDED.started_at,
 completed_at=EXCLUDED.completed_at,source='crossref',
 strategy=CASE WHEN acquisition_runs.strategy IN('citation_traversal','provider_refresh')
 THEN acquisition_runs.strategy ELSE 'citation_traversal' END;

CREATE TABLE acquisition_items (
 acquisition_id uuid NOT NULL REFERENCES acquisition_runs(id) ON DELETE CASCADE,
 project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 report_id uuid NOT NULL REFERENCES reports(id), canonical_doi text NOT NULL,
 depth integer NOT NULL,parent_doi text,
 status text NOT NULL DEFAULT 'queued' CHECK(status IN('queued','fetching','fetched','skipped','not_found','failed')),
 attempts integer NOT NULL DEFAULT 0,last_error text,
 queued_at timestamptz NOT NULL DEFAULT now(),fetched_at timestamptz,work_event_id uuid,
 PRIMARY KEY(acquisition_id,canonical_doi)
);
INSERT INTO acquisition_items
SELECT i.ingestion_id,i.project_id,m.report_id,i.canonical_doi,i.depth,i.parent_doi,
 i.status,i.attempts,i.last_error,i.queued_at,i.fetched_at,i.work_event_id
FROM ingestion_items i JOIN doi_report_map m ON m.doi=lower(i.canonical_doi);
CREATE INDEX acquisition_items_status_idx ON acquisition_items(acquisition_id,status);
CREATE INDEX acquisition_items_nonterminal_idx ON acquisition_items(acquisition_id,queued_at)
 WHERE status IN('queued','fetching');

UPDATE acquisition_runs a SET config=jsonb_build_object(
 'seed_dois',COALESCE((SELECT jsonb_agg(doi ORDER BY doi) FROM (
  SELECT lower(i.canonical_doi) doi FROM acquisition_items i WHERE i.acquisition_id=a.id AND i.depth=0
  UNION SELECT lower(pw.canonical_doi) FROM project_works pw
  WHERE pw.first_seen_ingestion_id=a.id AND pw.seed
 ) seeds),'[]'::jsonb),'max_depth',a.max_depth,
 'metadata_provider',a.metadata_provider,'citation_provider',a.citation_provider)
WHERE a.source='crossref' AND NOT(a.config ? 'seed_dois');

-- Global provider success must not be inferred from one run's failed item.
CREATE TABLE report_fetch_state (
 report_id uuid PRIMARY KEY REFERENCES reports(id) ON DELETE CASCADE,
 canonical_doi text NOT NULL,provider text NOT NULL DEFAULT 'crossref',
 status text NOT NULL DEFAULT 'queued' CHECK(status IN('queued','fetching','fetched','not_found','failed')),
 locked_at timestamptz,fetched_at timestamptz,last_error text,updated_at timestamptz NOT NULL DEFAULT now(),
 owner_token uuid,lease_expires_at timestamptz,heartbeat_at timestamptz,attempts integer NOT NULL DEFAULT 0
);
INSERT INTO report_fetch_state
SELECT DISTINCT ON(m.report_id) m.report_id,m.doi,'crossref',
 CASE WHEN w.fetch_status='fetched' OR f.status='fetched' THEN 'fetched' ELSE COALESCE(f.status,'queued') END,
 f.locked_at,COALESCE(f.fetched_at,w.fetched_at),f.last_error,COALESCE(f.updated_at,now()),
 f.owner_token,f.lease_expires_at,f.heartbeat_at,COALESCE(f.attempts,0)
FROM doi_report_map m LEFT JOIN doi_fetch_state f ON lower(f.canonical_doi)=m.doi
LEFT JOIN works w ON lower(w.canonical_doi)=m.doi
ORDER BY m.report_id,(w.fetch_status='fetched' OR f.status='fetched') DESC NULLS LAST,
 COALESCE(f.fetched_at,w.fetched_at) DESC NULLS LAST,m.doi;
CREATE INDEX report_fetch_state_doi_idx ON report_fetch_state(canonical_doi);
CREATE INDEX report_fetch_state_stale_idx ON report_fetch_state(lease_expires_at) WHERE status='fetching';

INSERT INTO records(id,project_id,report_id,source,source_key,title,abstract_text,publication_year,raw,acquisition_run_id)
SELECT md5('deepref:record:'||pw.project_id::text||':'||pw.canonical_doi)::uuid,
 pw.project_id,m.report_id,'doi',pw.canonical_doi,w.title,w.abstract_text,
 COALESCE(w.published_year,w.issued_year),jsonb_build_object('doi',pw.canonical_doi,'seed',pw.seed,'min_depth',pw.min_depth),
 pw.first_seen_ingestion_id
FROM project_works pw JOIN doi_report_map m ON m.doi=lower(pw.canonical_doi)
JOIN works w ON w.canonical_doi=pw.canonical_doi
ON CONFLICT(id) DO UPDATE SET acquisition_run_id=COALESCE(records.acquisition_run_id,EXCLUDED.acquisition_run_id),
 raw=records.raw||EXCLUDED.raw;
INSERT INTO project_reports(project_id,report_id,first_seen_record_id)
SELECT r.project_id,r.report_id,r.id FROM records r WHERE r.report_id IS NOT NULL
ON CONFLICT(project_id,report_id) DO UPDATE SET first_seen_record_id=COALESCE(project_reports.first_seen_record_id,EXCLUDED.first_seen_record_id);

-- Every DOI observation survives even when multiple aliases identify one report.
INSERT INTO records(id,project_id,report_id,source,source_key,raw,acquisition_run_id)
SELECT md5('deepref:item:'||i.acquisition_id::text||':'||i.canonical_doi)::uuid,
 i.project_id,i.report_id,'doi_acquisition_item',i.acquisition_id::text||':'||i.canonical_doi,
 jsonb_build_object('doi',i.canonical_doi,'depth',i.depth,'parent_doi',i.parent_doi),i.acquisition_id
FROM acquisition_items i ON CONFLICT(project_id,source,source_key) DO NOTHING;
INSERT INTO record_provenance(record_id,acquisition_run_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,last_error,work_event_id)
SELECT r.id,i.acquisition_id,i.canonical_doi,i.depth,i.parent_doi,i.status,i.attempts,
 i.queued_at,i.fetched_at,i.last_error,i.work_event_id FROM acquisition_items i
JOIN records r ON r.project_id=i.project_id AND r.source='doi_acquisition_item'
 AND r.source_key=i.acquisition_id::text||':'||i.canonical_doi
ON CONFLICT(acquisition_run_id,canonical_doi) DO NOTHING;

INSERT INTO project_reports(project_id,report_id,first_seen_record_id)
SELECT r.project_id,r.report_id,r.id FROM records r
WHERE r.source='doi_acquisition_item' AND r.report_id IS NOT NULL
ON CONFLICT(project_id,report_id) DO NOTHING;

DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM legacy_citations c
 JOIN doi_report_map s ON s.doi=lower(c.source_doi)
 JOIN doi_report_map t ON t.doi=lower(c.target_doi)
 GROUP BY c.project_id,s.report_id,t.report_id HAVING count(*)>1) THEN
 RAISE EXCEPTION 'DOI alias citation collision: resolve provenance before cutover';
 END IF;
END $$;
INSERT INTO citations(project_id,source_report_id,target_report_id,source,first_seen_ingestion_id,legacy_source_doi,legacy_target_doi,created_at)
SELECT c.project_id,s.report_id,t.report_id,c.source,c.first_seen_ingestion_id,c.source_doi,c.target_doi,c.created_at
FROM legacy_citations c JOIN doi_report_map s ON s.doi=lower(c.source_doi)
JOIN doi_report_map t ON t.doi=lower(c.target_doi)
ON CONFLICT(project_id,source_report_id,target_report_id) DO UPDATE SET
 first_seen_ingestion_id=COALESCE(citations.first_seen_ingestion_id,EXCLUDED.first_seen_ingestion_id),
 created_at=LEAST(citations.created_at,EXCLUDED.created_at);

CREATE TABLE project_unresolved_references (
 id text NOT NULL,project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 source_report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
 raw_unstructured text,article_title text,author text,year text,volume text,first_page text,
 created_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(project_id,id)
);
INSERT INTO project_unresolved_references
SELECT u.id,u.project_id,m.report_id,u.raw_unstructured,u.article_title,u.author,u.year,u.volume,u.first_page,u.created_at
FROM unresolved_references u JOIN doi_report_map m ON m.doi=lower(u.source_doi);

UPDATE acquisition_runs a SET queued_count=c.queued,fetched_count=c.fetched,failed_count=c.failed,
 status=CASE WHEN a.status='cancelled' THEN 'cancelled' WHEN c.queued>0 THEN a.status
 WHEN c.fetched=0 AND c.failed>0 THEN 'failed' ELSE 'completed' END,
 completed_at=CASE WHEN c.queued=0 THEN COALESCE(a.completed_at,now()) ELSE a.completed_at END
FROM (SELECT a.id,count(i.*) FILTER(WHERE i.status IN('queued','fetching'))::integer queued,
 count(i.*) FILTER(WHERE i.status='fetched')::integer fetched,
 count(i.*) FILTER(WHERE i.status IN('failed','not_found'))::integer failed
 FROM acquisition_runs a LEFT JOIN acquisition_items i ON i.acquisition_id=a.id
 WHERE a.source='crossref' GROUP BY a.id)c WHERE a.id=c.id;

-- Only payload identity changes: queue ID, dedupe, attempts, ownership and lease stay intact.
UPDATE jobs SET payload=jsonb_set(payload,'{payload}',
 (payload->'payload'-'ingestion_id')||jsonb_build_object('acquisition_id',payload->'payload'->'ingestion_id'))
WHERE kind='work_fetch_requested' AND payload->'payload' ? 'ingestion_id';
UPDATE jobs SET payload=(payload-'ingestion_id')||jsonb_build_object('acquisition_id',payload->'ingestion_id')
WHERE kind='work_fetch_requested' AND payload ? 'ingestion_id';
-- Normalize historical flat payloads at the migration boundary, never at runtime.
UPDATE jobs j SET payload=jsonb_build_object(
 'schema_version',1,'event_id',j.id,'event_type','work.fetch.requested.v1',
 'occurred_at',j.created_at,'producer','deepref.migration','correlation_id',a.id,
 'causation_id',NULL,'entity_type','work','entity_key',i.canonical_doi,'revision',0,
 'payload',jsonb_build_object('project_id',i.project_id,'acquisition_id',a.id,
 'doi',i.canonical_doi,'depth',i.depth,'max_depth',a.max_depth,'parent_doi',i.parent_doi)
 || CASE WHEN j.payload ? 'payload' THEN j.payload->'payload' ELSE j.payload END)
FROM acquisition_items i JOIN acquisition_runs a ON a.id=i.acquisition_id
WHERE j.kind='work_fetch_requested' AND j.id=i.work_event_id AND NOT(j.payload ? 'event_id');

-- Recover mapped event claims lacking a queue row; existing queue leases win.
INSERT INTO jobs(id,project_id,kind,payload,state,attempts,max_attempts,leased_until,lease_owner,
 dedupe_key,last_error,created_at,completed_at)
SELECT i.work_event_id,i.project_id,'work_fetch_requested',jsonb_build_object(
 'schema_version',1,'event_id',i.work_event_id,'event_type','work.fetch.requested.v1',
 'occurred_at',i.queued_at,'producer','deepref.migration','correlation_id',i.acquisition_id,
 'causation_id',NULL,'entity_type','work','entity_key',i.canonical_doi,'revision',0,
 'payload',jsonb_build_object('project_id',i.project_id,'acquisition_id',i.acquisition_id,
 'doi',i.canonical_doi,'depth',i.depth,'max_depth',a.max_depth,'parent_doi',i.parent_doi)),
 CASE WHEN p.completed_at IS NOT NULL OR i.status IN('fetched','skipped','failed','not_found') THEN 'completed'
 WHEN p.owner_token IS NOT NULL AND p.lease_expires_at>now() THEN 'running' ELSE 'queued' END,
 COALESCE(p.attempts,i.attempts),GREATEST(5,COALESCE(p.attempts,i.attempts)+1),
 p.lease_expires_at,p.owner_token::text,'work_fetch:'||i.work_event_id::text,
 COALESCE(p.last_error,i.last_error),i.queued_at,p.completed_at
FROM acquisition_items i JOIN acquisition_runs a ON a.id=i.acquisition_id
LEFT JOIN processed_events p ON p.event_id=i.work_event_id
WHERE i.work_event_id IS NOT NULL
ON CONFLICT(id) DO NOTHING;
UPDATE jobs SET payload=jsonb_build_object(
 'project_id',COALESCE(payload#>'{payload,data,project_id}',payload#>'{payload,project_id}'),
 'revision',payload->'revision')
WHERE kind='recompute_metrics' AND payload ? 'event_id';

ALTER TABLE projects ADD COLUMN metric_requested_revision bigint NOT NULL DEFAULT 0 CHECK(metric_requested_revision>=0);
UPDATE projects p SET metric_requested_revision=GREATEST(
 COALESCE((SELECT max(revision) FROM domain_events WHERE entity_type='metric' AND entity_key=p.id::text),0),
 COALESCE((SELECT max(revision) FROM metric_snapshots WHERE project_id=p.id),0),
 COALESCE((SELECT max(GREATEST(revision,watermark)) FROM projection_state WHERE project_id=p.id),0));

DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM processed_events p LEFT JOIN jobs j ON j.id=p.event_id
 WHERE p.completed_at IS NULL AND j.id IS NULL) THEN
 RAISE EXCEPTION 'Unmapped pending event claim: cannot retire execution identity'; END IF;
 IF EXISTS(SELECT 1 FROM dead_letter_records d LEFT JOIN jobs j ON j.id=d.source_event_id
 WHERE d.source_event_id IS NOT NULL AND j.id IS NULL) THEN
 RAISE EXCEPTION 'Unmapped dead-letter job: cannot retire retry identity'; END IF;
 IF (SELECT count(*) FROM ingestion_items)<>(SELECT count(*) FROM acquisition_items) THEN
 RAISE EXCEPTION 'Acquisition item coverage mismatch'; END IF;
 IF (SELECT count(*) FROM unresolved_references)<>(SELECT count(*) FROM project_unresolved_references) THEN
 RAISE EXCEPTION 'Unresolved reference coverage mismatch'; END IF;
 IF EXISTS(SELECT 1 FROM ingestions i LEFT JOIN acquisition_runs a ON a.id=i.id WHERE a.id IS NULL) THEN
 RAISE EXCEPTION 'Acquisition run coverage mismatch'; END IF;
END $$;
