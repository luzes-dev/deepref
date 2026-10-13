-- One lifecycle, with typed domain detail and the existing jobs lease authority.
LOCK TABLE automation_runs, workflow_runs, automation_step_runs, workflow_node_runs,
  review_step_attempts, jobs IN SHARE ROW EXCLUSIVE MODE;
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM automation_runs a JOIN workflow_runs w USING(id)) THEN
    RAISE EXCEPTION 'durable run UUID collision';
  END IF;
  IF EXISTS (SELECT 1 FROM automation_step_runs a JOIN workflow_node_runs w USING(id)) THEN
    RAISE EXCEPTION 'durable step UUID collision';
  END IF;
  IF EXISTS (SELECT 1 FROM review_run_manifests m LEFT JOIN automation_runs r
    ON r.id=m.automation_run_id AND r.project_id=m.project_id WHERE r.id IS NULL) THEN
    RAISE EXCEPTION 'orphan review manifest';
  END IF;
END $$;

CREATE TABLE durable_runs (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  kind text NOT NULL CHECK(kind IN ('automation','workflow')),
  job_id uuid,
  actor_kind text NOT NULL CHECK(actor_kind IN ('user','automation','system')),
  actor_id text NOT NULL,
  idempotency_key text NOT NULL,
  status text NOT NULL CHECK(status IN ('queued','running','waiting','blocked','completed','failed','cancelled')),
  error text,
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  UNIQUE(project_id,id),
  FOREIGN KEY(project_id,job_id) REFERENCES jobs(project_id,id) ON DELETE RESTRICT
);
CREATE INDEX durable_runs_project_created_idx ON durable_runs(project_id,created_at DESC,id DESC);
CREATE INDEX durable_runs_active_idx ON durable_runs(status) WHERE status IN ('queued','running','waiting');

DROP FUNCTION list_automation_runs(uuid,integer);
DROP FUNCTION get_automation_run(uuid,uuid);
DROP VIEW automation_run_details;
CREATE TABLE automation_run_details (
  id uuid PRIMARY KEY REFERENCES durable_runs(id) ON DELETE CASCADE,
  project_id uuid NOT NULL,
  definition_id uuid NOT NULL,
  recipe_id text NOT NULL,
  recipe_version integer NOT NULL,
  trigger_kind text NOT NULL,
  trigger_reference text,
  idempotency_key text NOT NULL,
  UNIQUE(project_id,definition_id,trigger_kind,idempotency_key),
  FOREIGN KEY(project_id,id) REFERENCES durable_runs(project_id,id) ON DELETE CASCADE,
  FOREIGN KEY(project_id,definition_id) REFERENCES automation_definitions(project_id,id) ON DELETE RESTRICT
);
CREATE TABLE workflow_run_details (
  id uuid PRIMARY KEY REFERENCES durable_runs(id) ON DELETE CASCADE,
  project_id uuid NOT NULL,
  workflow_id uuid NOT NULL,
  version_id uuid,
  graph jsonb NOT NULL,
  trigger_kind text NOT NULL,
  trigger_data jsonb NOT NULL,
  idempotency_key text NOT NULL,
  test_mode boolean NOT NULL,
  UNIQUE(workflow_id,idempotency_key),
  FOREIGN KEY(project_id,id) REFERENCES durable_runs(project_id,id) ON DELETE CASCADE,
  FOREIGN KEY(project_id,workflow_id) REFERENCES workflows(project_id,id) ON DELETE CASCADE,
  FOREIGN KEY(workflow_id,version_id) REFERENCES workflow_versions(workflow_id,id) ON DELETE CASCADE
);
INSERT INTO durable_runs
SELECT id,project_id,'automation',job_id,actor_kind,actor_id,idempotency_key,status,error,created_at,started_at,finished_at FROM automation_runs;
-- Review-specific blocked state is the canonical state, not another lifecycle.
UPDATE durable_runs r SET status=m.state,started_at=m.started_at,finished_at=m.finished_at,
  error=CASE WHEN m.state IN ('failed','blocked') THEN m.state_message ELSE r.error END
FROM review_run_manifests m WHERE m.automation_run_id=r.id;
INSERT INTO durable_runs
SELECT id,project_id,'workflow',NULL,actor_kind,actor_id,idempotency_key,status,error,created_at,started_at,finished_at FROM workflow_runs;
INSERT INTO automation_run_details SELECT id,project_id,definition_id,recipe_id,recipe_version,trigger_kind,trigger_reference,idempotency_key FROM automation_runs;
INSERT INTO workflow_run_details SELECT id,project_id,workflow_id,version_id,graph,trigger_kind,trigger_data,idempotency_key,test_mode FROM workflow_runs;

CREATE TABLE durable_steps (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL,
  run_id uuid NOT NULL,
  handler_kind text NOT NULL CHECK(handler_kind IN ('automation','workflow','review')),
  step_key text NOT NULL,
  ordinal integer,
  status text NOT NULL CHECK(status IN ('pending','queued','running','waiting','blocked','completed','failed','skipped','cancelled')),
  attempts integer NOT NULL DEFAULT 0 CHECK(attempts>=0),
  claimed_by text,
  claimed_job_id uuid,
  claimed_job_attempt integer,
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  error text,
  output jsonb,
  UNIQUE(project_id,id),
  FOREIGN KEY(project_id,run_id) REFERENCES durable_runs(project_id,id) ON DELETE CASCADE,
  FOREIGN KEY(project_id,claimed_job_id) REFERENCES jobs(project_id,id),
  CHECK ((claimed_job_id IS NULL)=(claimed_job_attempt IS NULL)),
  CHECK (claimed_job_attempt IS NULL OR claimed_job_attempt>0)
);
CREATE INDEX durable_steps_run_idx ON durable_steps(run_id,ordinal,created_at);
CREATE TABLE durable_attempts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL,
  run_id uuid NOT NULL,
  step_id uuid NOT NULL,
  attempt_number integer NOT NULL CHECK(attempt_number>0),
  status text NOT NULL CHECK(status IN ('running','waiting','completed','failed','blocked','cancelled','skipped')),
  worker_id text,
  job_id uuid,
  job_attempt integer,
  started_at timestamptz NOT NULL DEFAULT now(),
  finished_at timestamptz,
  error text,
  UNIQUE(step_id,attempt_number),
  FOREIGN KEY(project_id,run_id) REFERENCES durable_runs(project_id,id) ON DELETE CASCADE,
  FOREIGN KEY(project_id,step_id) REFERENCES durable_steps(project_id,id) ON DELETE CASCADE,
  FOREIGN KEY(project_id,job_id) REFERENCES jobs(project_id,id),
  CHECK((job_id IS NULL)=(job_attempt IS NULL))
);
CREATE TABLE automation_step_details (
  id uuid PRIMARY KEY REFERENCES durable_steps(id) ON DELETE CASCADE,
  project_id uuid NOT NULL,
  run_id uuid NOT NULL,
  ordinal integer NOT NULL,
  step_key text NOT NULL,
  step_kind text NOT NULL,
  accepted_attempt_id uuid REFERENCES durable_attempts(id) DEFERRABLE INITIALLY DEFERRED,
  input_fingerprint text,
  UNIQUE(project_id,run_id,ordinal),
  UNIQUE(project_id,run_id,step_key),
  FOREIGN KEY(project_id,run_id) REFERENCES durable_runs(project_id,id) ON DELETE CASCADE
);
CREATE TABLE workflow_step_details (
  id uuid PRIMARY KEY REFERENCES durable_steps(id) ON DELETE CASCADE,
  run_id uuid NOT NULL REFERENCES durable_runs(id) ON DELETE CASCADE,
  node_id text NOT NULL,
  iteration text NOT NULL DEFAULT '',
  node_type text NOT NULL,
  input jsonb,
  fired_ports text[] NOT NULL DEFAULT '{}',
  items integer NOT NULL DEFAULT 0 CHECK(items>=0),
  note text,
  wait_state jsonb,
  UNIQUE(run_id,node_id,iteration)
);
INSERT INTO durable_steps
SELECT s.id,s.project_id,s.automation_run_id,'automation',s.step_key,s.ordinal,
 CASE WHEN s.status='pending' THEN 'queued' ELSE s.status END,s.attempts,
 s.claimed_by,CASE WHEN s.claimed_job_attempt IS NOT NULL THEN r.job_id END,s.claimed_job_attempt,
 r.created_at,s.started_at,s.finished_at,s.error,s.output
FROM automation_step_runs s JOIN automation_runs r ON r.id=s.automation_run_id;
INSERT INTO durable_steps
SELECT s.id,s.project_id,s.run_id,'workflow',s.node_id,NULL,s.status,s.attempts,
 j.lease_owner,s.claimed_job_id,s.claimed_attempt,s.created_at,s.started_at,s.finished_at,s.error,s.output
FROM workflow_node_runs s LEFT JOIN jobs j ON j.id=s.claimed_job_id;
INSERT INTO workflow_step_details SELECT id,run_id,node_id,iteration,node_type,input,fired_ports,items,note,wait_state FROM workflow_node_runs;

-- Each compiled review node has one canonical step. Existing attempt IDs are retained.
INSERT INTO durable_steps(id,project_id,run_id,handler_kind,step_key,status,attempts,started_at,finished_at,error)
SELECT automation_uuid_from_text('durable-review-step',project_id::text||':'||automation_run_id::text||':'||node_id),
 project_id,automation_run_id,'review',node_id,
 (array_agg(status ORDER BY attempt_number DESC))[1],max(attempt_number),min(started_at),max(finished_at),
 (array_agg(error_message ORDER BY attempt_number DESC))[1]
FROM review_step_attempts GROUP BY project_id,automation_run_id,node_id;
INSERT INTO durable_attempts(id,project_id,run_id,step_id,attempt_number,status,worker_id,job_id,job_attempt,started_at,finished_at,error)
SELECT a.id,a.project_id,a.automation_run_id,s.id,a.attempt_number,a.status,a.worker_id,
 CASE WHEN a.status='running' THEN r.job_id END,
 CASE WHEN a.status='running' THEN j.attempts END,
 a.started_at,a.finished_at,a.error_message
FROM review_step_attempts a JOIN durable_steps s ON s.run_id=a.automation_run_id AND s.handler_kind='review' AND s.step_key=a.node_id
JOIN durable_runs r ON r.id=a.automation_run_id JOIN jobs j ON j.id=r.job_id;
CREATE TABLE review_attempt_details (
 id uuid PRIMARY KEY REFERENCES durable_attempts(id) ON DELETE CASCADE,
 project_id uuid NOT NULL,
 run_id uuid NOT NULL REFERENCES durable_runs(id) ON DELETE CASCADE,
 node_id text NOT NULL,
 node_version integer NOT NULL CHECK(node_version>0),
 input_fingerprint text NOT NULL,
 artifact_id uuid,
 model_run_id uuid,
 error_code text,
 error_message text,
 accepted_at timestamptz,
 FOREIGN KEY(project_id,artifact_id) REFERENCES review_artifacts(project_id,id) DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(model_run_id,project_id) REFERENCES ai_runs(id,project_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE UNIQUE INDEX review_attempt_details_accepted_fingerprint_uq ON review_attempt_details(project_id,node_id,input_fingerprint) WHERE accepted_at IS NOT NULL;
INSERT INTO review_attempt_details SELECT id,project_id,automation_run_id,node_id,node_version,input_fingerprint,artifact_id,model_run_id,error_code,error_message,accepted_at FROM review_step_attempts;
INSERT INTO automation_step_details SELECT id,project_id,automation_run_id,ordinal,step_key,step_kind,accepted_attempt_id,input_fingerprint FROM automation_step_runs;
-- Legacy recipe/workflow histories recorded only a count; preserve that count and
-- the known latest transition, without inventing earlier timing or identities.
INSERT INTO durable_attempts(project_id,run_id,step_id,attempt_number,status,worker_id,job_id,job_attempt,started_at,finished_at,error)
SELECT project_id,run_id,id,attempts,
 CASE WHEN status IN ('pending','queued') THEN 'failed' ELSE status END,
 claimed_by,claimed_job_id,claimed_job_attempt,COALESCE(started_at,created_at),finished_at,error
FROM durable_steps WHERE handler_kind IN ('automation','workflow') AND attempts>0;

DO $$ BEGIN
 IF (SELECT count(*) FROM durable_runs)<>(SELECT count(*) FROM automation_runs)+(SELECT count(*) FROM workflow_runs)
 OR (SELECT count(*) FROM automation_step_details)<>(SELECT count(*) FROM automation_step_runs)
 OR (SELECT count(*) FROM workflow_step_details)<>(SELECT count(*) FROM workflow_node_runs)
 OR (SELECT count(*) FROM review_attempt_details)<>(SELECT count(*) FROM review_step_attempts) THEN
  RAISE EXCEPTION 'durable lifecycle source coverage mismatch';
 END IF;
END $$;
