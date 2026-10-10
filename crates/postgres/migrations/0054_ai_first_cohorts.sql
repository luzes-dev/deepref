-- AI-first is a reversible routing workflow, independent of per-record autonomy.
CREATE TABLE project_ai_screening_authority (
 project_id uuid PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
 stage text NOT NULL DEFAULT 'title_abstract' CHECK(stage='title_abstract'),
 ceiling text NOT NULL CHECK(ceiling IN ('off','routing','cohort_finalization')),
 suspended_reason text,
 approved_by text NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now(),
 next_audit_ordinal integer NOT NULL DEFAULT 1 CHECK(next_audit_ordinal BETWEEN 1 AND 26)
);
CREATE TABLE ai_screening_cohorts (
 id uuid PRIMARY KEY, project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 stage text NOT NULL DEFAULT 'title_abstract' CHECK(stage='title_abstract'),
 protocol_version_id uuid NOT NULL REFERENCES protocol_versions(id) ON DELETE CASCADE,
 semantic_bundle_hash text NOT NULL CHECK(length(semantic_bundle_hash)=64),
 semantic_identity jsonb NOT NULL,
 policy_version integer NOT NULL CHECK(policy_version=1),
 status text NOT NULL DEFAULT 'open' CHECK(status IN
 ('open','closed','auditing','passed','failed','invalidated','declined','finalized')),
 target_percent integer NOT NULL CHECK(target_percent IN (95,98)),
 audit_ordinal integer, alpha_billionths integer CHECK(alpha_billionths BETWEEN 1 AND 50000000),
 reference_relevant integer CHECK(reference_relevant>=0),
 population integer CHECK(population>=0), sample_size integer CHECK(sample_size>=0),
 sampling_seed uuid, sampling_algorithm text,
 reference_snapshot jsonb, frame_snapshot jsonb, result jsonb,
 approved_by text NOT NULL, final_approval_id uuid, finalized_by text,
 created_at timestamptz NOT NULL DEFAULT now(), closed_at timestamptz,
 evaluated_at timestamptz, finalized_at timestamptz, invalidation_reason text,
 UNIQUE(project_id,id), UNIQUE(project_id,audit_ordinal),
 CHECK((sampling_seed IS NULL AND audit_ordinal IS NULL AND sample_size IS NULL)
   OR (sampling_seed IS NOT NULL AND audit_ordinal IS NOT NULL AND sample_size IS NOT NULL
       AND population IS NOT NULL AND reference_relevant IS NOT NULL
       AND alpha_billionths IS NOT NULL AND sampling_algorithm IS NOT NULL
       AND sample_size<=population)),
 CHECK((final_approval_id IS NULL)=(finalized_by IS NULL))
);
CREATE UNIQUE INDEX ai_first_one_active ON ai_screening_cohorts(project_id)
 WHERE status IN ('open','closed','auditing','passed');
CREATE TABLE ai_screening_cohort_members (
 project_id uuid NOT NULL, cohort_id uuid NOT NULL, report_id uuid NOT NULL,
 evaluated_revision bigint NOT NULL CHECK(evaluated_revision>=0),
 review_run_id uuid, sampled boolean NOT NULL DEFAULT false, quarantine_frame boolean NOT NULL DEFAULT false,
 PRIMARY KEY(cohort_id,report_id),
 FOREIGN KEY(project_id,cohort_id) REFERENCES ai_screening_cohorts(project_id,id) ON DELETE CASCADE,
 FOREIGN KEY(project_id,report_id) REFERENCES project_reports(project_id,report_id) ON DELETE CASCADE
);
CREATE TABLE ai_screening_dispositions (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(), project_id uuid NOT NULL,
 cohort_id uuid NOT NULL, report_id uuid NOT NULL,
 stage text NOT NULL DEFAULT 'title_abstract' CHECK(stage='title_abstract'),
 kind text NOT NULL DEFAULT 'quarantine_exclude' CHECK(kind='quarantine_exclude'),
 review_run_id uuid NOT NULL, ai_run_id uuid NOT NULL,
 evaluated_revision bigint NOT NULL CHECK(evaluated_revision>=0),
 protocol_version_id uuid NOT NULL REFERENCES protocol_versions(id) ON DELETE CASCADE,
 semantic_bundle_hash text NOT NULL CHECK(length(semantic_bundle_hash)=64),
 policy_version integer NOT NULL CHECK(policy_version=1),
 source_snapshot jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(), voided_at timestamptz, void_reason text,
 finalized_at timestamptz, finalized_event_id uuid REFERENCES screening_events(id) ON DELETE CASCADE,
 FOREIGN KEY(cohort_id,report_id) REFERENCES ai_screening_cohort_members(cohort_id,report_id) ON DELETE CASCADE,
 FOREIGN KEY(project_id,cohort_id) REFERENCES ai_screening_cohorts(project_id,id) ON DELETE CASCADE,
 CHECK(NOT (voided_at IS NOT NULL AND finalized_at IS NOT NULL)),
 CHECK((voided_at IS NULL)=(void_reason IS NULL)),
 CHECK((finalized_at IS NULL)=(finalized_event_id IS NULL))
);
CREATE UNIQUE INDEX ai_first_active_disposition ON ai_screening_dispositions(project_id,report_id)
 WHERE voided_at IS NULL AND finalized_at IS NULL;
CREATE TABLE ai_screening_audit_labels (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(), project_id uuid NOT NULL,
 cohort_id uuid NOT NULL, report_id uuid NOT NULL, actor_id text NOT NULL,
 decision text NOT NULL CHECK(decision IN ('include','exclude','maybe')),
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 UNIQUE(cohort_id,report_id,actor_id),
 FOREIGN KEY(cohort_id,report_id) REFERENCES ai_screening_cohort_members(cohort_id,report_id) ON DELETE CASCADE,
 FOREIGN KEY(project_id,cohort_id) REFERENCES ai_screening_cohorts(project_id,id) ON DELETE CASCADE
);
CREATE FUNCTION protect_ai_first_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='ai_screening_audit_labels' THEN
   RAISE EXCEPTION 'audit labels are immutable';
 ELSIF TG_TABLE_NAME='ai_screening_dispositions' THEN
   IF (to_jsonb(NEW)-ARRAY['voided_at','void_reason','finalized_at','finalized_event_id'])
      IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['voided_at','void_reason','finalized_at','finalized_event_id'])
      OR OLD.voided_at IS NOT NULL OR OLD.finalized_at IS NOT NULL THEN
     RAISE EXCEPTION 'disposition evidence is immutable';
   END IF;
 ELSIF OLD.sampling_seed IS NOT NULL AND
   (to_jsonb(NEW)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by','finalized_at','invalidation_reason'])
   IS DISTINCT FROM
   (to_jsonb(OLD)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by','finalized_at','invalidation_reason']) THEN
   RAISE EXCEPTION 'sample design is frozen';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER ai_first_label_immutable BEFORE UPDATE ON ai_screening_audit_labels
 FOR EACH ROW EXECUTE FUNCTION protect_ai_first_evidence();
CREATE TRIGGER ai_first_disposition_immutable BEFORE UPDATE ON ai_screening_dispositions
 FOR EACH ROW EXECUTE FUNCTION protect_ai_first_evidence();
CREATE TRIGGER ai_first_design_frozen BEFORE UPDATE ON ai_screening_cohorts
 FOR EACH ROW EXECUTE FUNCTION protect_ai_first_evidence();

-- Never reveal AI opinions on a routing cohort before both reference labels.
ALTER FUNCTION ai_screening_proposal_withheld(uuid,uuid,uuid,text) RENAME TO ai_screening_proposal_withheld_legacy;
CREATE FUNCTION ai_first_blinded(project uuid, report uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m
  JOIN ai_screening_cohorts c ON c.id=m.cohort_id
  WHERE m.project_id=project AND m.report_id=report
    AND c.status IN ('open','closed','auditing','passed'))
$$;
CREATE FUNCTION ai_screening_proposal_withheld(project uuid,proposal uuid,report uuid,screening_stage text)
RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT (screening_stage='title_abstract' AND ai_first_blinded(project,report))
   OR ai_screening_proposal_withheld_legacy(project,proposal,report,screening_stage)
$$;
-- The legacy function was referenced by OID in the SQL availability function.
-- Wrap availability itself so no reveal trigger contaminates an audit label.
ALTER FUNCTION record_available_ai_opinions(uuid,uuid) RENAME TO record_available_ai_opinions_legacy;
CREATE FUNCTION record_available_ai_opinions(project uuid,report uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE target uuid;
BEGIN
 IF report IS NOT NULL THEN
   IF NOT ai_first_blinded(project,report) THEN
     PERFORM record_available_ai_opinions_legacy(project,report);
   END IF;
 ELSE
   FOR target IN SELECT report_id FROM project_reports WHERE project_id=project LOOP
     IF NOT ai_first_blinded(project,target) THEN
       PERFORM record_available_ai_opinions_legacy(project,target);
     END IF;
   END LOOP;
 END IF;
END $$;

CREATE FUNCTION ai_first_queue_visible(project uuid, report uuid, actor text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT NOT EXISTS(SELECT 1 FROM ai_screening_dispositions d
   JOIN ai_screening_cohorts c ON c.id=d.cohort_id
   JOIN ai_screening_cohort_members m ON m.cohort_id=c.id AND m.report_id=d.report_id
   WHERE d.project_id=project AND d.report_id=report AND d.voided_at IS NULL
     AND d.finalized_at IS NULL AND (
       c.status IN ('open','closed','passed') OR
       (c.status='auditing' AND (NOT m.sampled OR EXISTS(
         SELECT 1 FROM ai_screening_audit_labels l
         WHERE l.cohort_id=c.id AND l.report_id=report AND l.actor_id=actor)))))
$$;

CREATE FUNCTION protect_ai_first_frame() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM ai_screening_cohorts WHERE id=OLD.cohort_id AND sampling_seed IS NOT NULL)
    AND NEW IS DISTINCT FROM OLD THEN RAISE EXCEPTION 'audit frame is frozen'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER ai_first_frame_frozen BEFORE UPDATE ON ai_screening_cohort_members
 FOR EACH ROW EXECUTE FUNCTION protect_ai_first_frame();
CREATE FUNCTION ai_first_run_visible(project uuid,run uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT NOT EXISTS(SELECT 1 FROM review_run_manifests m
  JOIN ai_screening_cohorts c ON c.id=(m.origin->>'cohort_id')::uuid
  WHERE m.project_id=project AND m.automation_run_id=run AND m.origin->>'kind'='ai_first_triggered'
    AND c.status IN ('open','closed','auditing','passed'))
$$;

ALTER TABLE ai_screening_cohort_members ADD FOREIGN KEY(project_id,review_run_id)
 REFERENCES review_run_manifests(project_id,automation_run_id) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE ai_screening_dispositions ADD FOREIGN KEY(project_id,review_run_id)
 REFERENCES review_run_manifests(project_id,automation_run_id) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE ai_screening_dispositions ADD FOREIGN KEY(ai_run_id,project_id)
 REFERENCES ai_runs(id,project_id) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED;
