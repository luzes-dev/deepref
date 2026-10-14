-- Atomically retarget existing queue records; identity, leases and attempt budgets
-- are deliberately not changed. Terminal queue history is retained too.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM jobs j WHERE j.kind='automation_run' AND j.state IN ('queued','running')
   AND NOT EXISTS(SELECT 1 FROM durable_runs r JOIN durable_steps s ON s.run_id=r.id AND s.handler_kind='automation'
      WHERE r.job_id=j.id)) THEN RAISE EXCEPTION 'active automation job lacks durable target'; END IF;
 IF EXISTS(SELECT 1 FROM jobs j WHERE j.kind='workflow_step' AND j.state IN ('queued','running')
   AND NOT EXISTS(SELECT 1 FROM durable_steps s WHERE s.id=(j.payload->>'node_run_id')::uuid)) THEN
   RAISE EXCEPTION 'active workflow job lacks durable target'; END IF;
END $$;
UPDATE jobs j SET kind='durable_run_step',payload=jsonb_build_object('run_id',s.run_id,'step_id',s.id,'handler_kind','automation')
FROM durable_runs r JOIN LATERAL (
 SELECT id,run_id FROM durable_steps WHERE run_id=r.id AND handler_kind='automation'
 ORDER BY CASE WHEN status='running' THEN 0 WHEN status IN ('pending','queued','failed') THEN 1 ELSE 2 END,ordinal LIMIT 1
) s ON true WHERE j.id=r.job_id AND j.kind='automation_run';
UPDATE jobs j SET kind='durable_run_step',payload=jsonb_build_object('run_id',s.run_id,'step_id',s.id,'handler_kind','workflow')
FROM durable_steps s WHERE j.kind='workflow_step' AND s.id=(j.payload->>'node_run_id')::uuid;

-- Retarget every incoming FK using its original definition/options. This also
-- covers application-owned references added after the initial lifecycle schema.
DO $$ DECLARE c record; replacement text; definition text; BEGIN
 FOR c IN SELECT con.oid,con.conname,con.conrelid::regclass AS child,
   con.confrelid::regclass::text AS parent FROM pg_constraint con
   WHERE con.contype='f' AND con.confrelid IN ('automation_runs'::regclass,'workflow_runs'::regclass,
     'automation_step_runs'::regclass,'workflow_node_runs'::regclass,'review_step_attempts'::regclass)
   AND con.conrelid NOT IN ('automation_runs'::regclass,'workflow_runs'::regclass,
     'automation_step_runs'::regclass,'workflow_node_runs'::regclass,'review_step_attempts'::regclass)
 LOOP
  replacement:=CASE WHEN c.parent IN ('automation_runs','workflow_runs') THEN 'durable_runs'
   WHEN c.parent='review_step_attempts' THEN 'durable_attempts' ELSE 'durable_steps' END;
  definition:=replace(pg_get_constraintdef(c.oid),'REFERENCES '||c.parent||'(', 'REFERENCES '||replacement||'(');
  EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I',c.child,c.conname);
  EXECUTE format('ALTER TABLE %s ADD CONSTRAINT %I %s',c.child,c.conname,definition);
 END LOOP;
END $$;
ALTER TABLE second_review_attempts ADD CONSTRAINT second_review_attempts_durable_run_fk
 FOREIGN KEY(project_id,review_run_id) REFERENCES durable_runs(project_id,id);
CREATE OR REPLACE FUNCTION enforce_ai_run_parent_automation_project() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.parent_automation_run_id IS NOT NULL AND NOT EXISTS(
   SELECT 1 FROM durable_runs r WHERE r.id=NEW.parent_automation_run_id
     AND r.project_id=NEW.project_id AND r.kind='automation') THEN
  RAISE EXCEPTION 'AI run parent automation run belongs to another project' USING ERRCODE='foreign_key_violation';
 END IF;
 RETURN NEW;
END $$;
-- Manifest identity/generation/reuse and scientific data remain; lifecycle does not.
DO $$ DECLARE c record; BEGIN
 FOR c IN SELECT conname FROM pg_constraint WHERE conrelid='review_run_manifests'::regclass
  AND contype='c' AND pg_get_constraintdef(oid) ~ '\mstate\M'
 LOOP EXECUTE format('ALTER TABLE review_run_manifests DROP CONSTRAINT %I',c.conname); END LOOP;
END $$;
ALTER TABLE review_run_manifests DROP COLUMN state,DROP COLUMN started_at,DROP COLUMN finished_at;
-- Domain acceptance cannot be rewritten after it has become reusable evidence.
CREATE FUNCTION protect_review_attempt_detail() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.accepted_at IS NOT NULL AND NEW IS DISTINCT FROM OLD THEN
  RAISE EXCEPTION 'accepted review attempt is immutable' USING ERRCODE='integrity_constraint_violation';
 END IF;
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.project_id IS DISTINCT FROM OLD.project_id
   OR NEW.run_id IS DISTINCT FROM OLD.run_id OR NEW.node_id IS DISTINCT FROM OLD.node_id
   OR NEW.node_version IS DISTINCT FROM OLD.node_version
   OR NEW.input_fingerprint IS DISTINCT FROM OLD.input_fingerprint THEN
  RAISE EXCEPTION 'review attempt identity is immutable' USING ERRCODE='integrity_constraint_violation';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER review_attempt_detail_immutable BEFORE UPDATE ON review_attempt_details
 FOR EACH ROW EXECUTE FUNCTION protect_review_attempt_detail();
-- Remove obsolete lifecycle entry points rather than retain compatibility aliases.
DO $$ DECLARE f record; BEGIN
 FOR f IN SELECT oid::regprocedure AS signature FROM pg_proc WHERE pronamespace='public'::regnamespace
  AND proname IN ('dispatch_automation_trigger','start_automation_manually','enforce_automation_run_transition',
    'enforce_automation_step_transition','enforce_automation_run_snapshot','enforce_automation_step_snapshot')
 LOOP EXECUTE 'DROP FUNCTION IF EXISTS '||f.signature::text||' CASCADE'; END LOOP;
END $$;
DROP TABLE automation_step_runs;
DROP TABLE workflow_node_runs;
DROP TABLE review_step_attempts;
DROP TABLE automation_runs;
DROP TABLE workflow_runs;
DROP FUNCTION reject_terminal_review_attempt_changes();
-- Assert the clean cutover and its preserved foreign-key validity in-transaction.
DO $$ DECLARE c record; BEGIN
 IF EXISTS(SELECT 1 FROM pg_class WHERE relname IN ('automation_runs','automation_step_runs','workflow_runs','workflow_node_runs','review_step_attempts')
  AND relnamespace='public'::regnamespace) THEN RAISE EXCEPTION 'obsolete lifecycle store remains'; END IF;
 FOR c IN SELECT conrelid::regclass AS child,conname FROM pg_constraint
  WHERE contype='f' AND confrelid IN ('durable_runs'::regclass,'durable_steps'::regclass,'durable_attempts'::regclass)
 LOOP EXECUTE format('ALTER TABLE %s VALIDATE CONSTRAINT %I',c.child,c.conname); END LOOP;
END $$;

ALTER TABLE durable_steps ADD UNIQUE(project_id,run_id,id), ADD UNIQUE(run_id,id);
ALTER TABLE durable_attempts ADD UNIQUE(project_id,run_id,id);
ALTER TABLE automation_step_details ADD FOREIGN KEY(project_id,run_id,id)
 REFERENCES durable_steps(project_id,run_id,id) ON DELETE CASCADE;
ALTER TABLE workflow_step_details ADD FOREIGN KEY(run_id,id)
 REFERENCES durable_steps(run_id,id) ON DELETE CASCADE;
ALTER TABLE review_attempt_details ADD FOREIGN KEY(project_id,run_id,id)
 REFERENCES durable_attempts(project_id,run_id,id) ON DELETE CASCADE;

CREATE FUNCTION protect_durable_identity() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE mutable text[]; BEGIN
 mutable:=CASE TG_TABLE_NAME
  WHEN 'durable_runs' THEN ARRAY['status','error','started_at','finished_at']
  WHEN 'durable_steps' THEN ARRAY['status','attempts','claimed_by','claimed_job_id','claimed_job_attempt','started_at','finished_at','error','output']
  WHEN 'durable_attempts' THEN ARRAY['status','finished_at','error']
  WHEN 'automation_step_details' THEN ARRAY['accepted_attempt_id','input_fingerprint']
  WHEN 'workflow_step_details' THEN ARRAY['input','fired_ports','items','note','wait_state']
  ELSE ARRAY[]::text[] END;
 IF (to_jsonb(NEW)-mutable) IS DISTINCT FROM (to_jsonb(OLD)-mutable) THEN
  RAISE EXCEPTION 'durable identity and domain snapshot are immutable' USING ERRCODE='integrity_constraint_violation';
 END IF;
 IF TG_TABLE_NAME='durable_attempts' THEN
  IF OLD.status<>'running' AND NEW IS DISTINCT FROM OLD THEN
   RAISE EXCEPTION 'terminal durable attempt is immutable' USING ERRCODE='integrity_constraint_violation';
  END IF;
 END IF;
 IF TG_TABLE_NAME='automation_step_details' THEN
  IF OLD.accepted_attempt_id IS NOT NULL AND NEW IS DISTINCT FROM OLD THEN
   RAISE EXCEPTION 'accepted recipe evidence is immutable' USING ERRCODE='integrity_constraint_violation';
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER durable_runs_identity BEFORE UPDATE ON durable_runs FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
CREATE TRIGGER durable_steps_identity BEFORE UPDATE ON durable_steps FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
CREATE TRIGGER durable_attempts_identity BEFORE UPDATE ON durable_attempts FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
CREATE TRIGGER automation_run_details_identity BEFORE UPDATE ON automation_run_details FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
CREATE TRIGGER automation_step_details_identity BEFORE UPDATE ON automation_step_details FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
CREATE TRIGGER workflow_run_details_identity BEFORE UPDATE ON workflow_run_details FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
CREATE TRIGGER workflow_step_details_identity BEFORE UPDATE ON workflow_step_details FOR EACH ROW EXECUTE FUNCTION protect_durable_identity();
