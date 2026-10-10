-- Controls conceal the audit stratum. They consume labels but never enter k or X.
ALTER TABLE ai_screening_cohort_members ADD COLUMN audit_control boolean NOT NULL DEFAULT false, ADD COLUMN audit_order integer CHECK(audit_order>=0);
ALTER TABLE ai_screening_cohort_members ADD CHECK(NOT (audit_control AND quarantine_frame));

CREATE FUNCTION ai_first_audit_masked(project uuid, report uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m
 JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE m.project_id=project AND m.report_id=report AND c.status='auditing')
$$;

CREATE FUNCTION ai_first_fresh_auditor(cohort uuid, actor text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT NOT EXISTS(SELECT 1 FROM ai_screening_cohorts c WHERE c.id=cohort AND (
 c.approved_by=actor OR EXISTS(SELECT 1 FROM ai_screening_cohort_members m JOIN screening_events e
 ON e.project_id=m.project_id AND e.report_id=m.report_id
 WHERE m.cohort_id=c.id AND e.actor_kind='user' AND e.actor_id=actor)
 OR EXISTS(SELECT 1 FROM ai_screening_cohort_members m JOIN ai_opinion_exposures e
 ON e.project_id=m.project_id AND e.report_id=m.report_id
 WHERE m.cohort_id=c.id
 AND (e.audience_actor_id IS NULL OR (e.audience_actor_kind='user' AND e.audience_actor_id=actor)))))
$$;

CREATE OR REPLACE FUNCTION ai_first_queue_visible(project uuid, report uuid, actor text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT CASE WHEN ai_first_audit_masked(project,report) THEN EXISTS(
 SELECT 1 FROM ai_screening_cohort_members m JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE m.project_id=project AND m.report_id=report AND c.status='auditing'
 AND (m.sampled OR m.audit_control) AND ai_first_fresh_auditor(c.id,actor)
 AND NOT EXISTS(SELECT 1 FROM ai_screening_audit_labels l WHERE l.cohort_id=c.id AND l.report_id=report AND l.actor_id=actor)
 AND (SELECT count(*) FROM ai_screening_audit_labels l WHERE l.cohort_id=c.id AND l.report_id=report)<2)
 ELSE NOT EXISTS(SELECT 1 FROM ai_screening_dispositions d
 JOIN ai_screening_cohorts c ON c.id=d.cohort_id
 WHERE d.project_id=project AND d.report_id=report AND d.voided_at IS NULL
 AND d.finalized_at IS NULL AND c.status IN ('open','closed','passed')) END
$$;

-- Older drawn cohorts have no controls and cannot satisfy the new frozen frame.
-- Release routing; preserve historical inference and canonical event provenance.
UPDATE ai_screening_cohorts SET status='invalidated',invalidation_reason='blind_controls_required'
 WHERE status IN ('auditing','passed');
UPDATE ai_screening_dispositions d SET voided_at=clock_timestamp(),void_reason='blind_controls_required'
 FROM ai_screening_cohorts c WHERE c.id=d.cohort_id AND c.invalidation_reason='blind_controls_required'
 AND d.voided_at IS NULL AND d.finalized_at IS NULL;

-- Default queue sorts must mix tasks rather than reproducing AI routing strata.
CREATE FUNCTION ai_first_queue_time(project uuid, report uuid, original timestamptz) RETURNS timestamptz LANGUAGE sql STABLE AS $$
 SELECT COALESCE((SELECT '2000-01-01 UTC'::timestamptz + m.audit_order * interval '1 microsecond'
 FROM ai_screening_cohort_members m JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE m.project_id=project AND m.report_id=report AND c.status='auditing'),original)
$$;

-- Prior user-requested model runs and their cached artifacts also reveal opinions.
-- Withhold project run inspection uniformly while the blind audit is active.
ALTER FUNCTION ai_first_run_visible(uuid,uuid) RENAME TO ai_first_run_visible_legacy;
CREATE FUNCTION ai_first_run_visible(project uuid,run uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT ai_first_run_visible_legacy(project,run)
 AND NOT EXISTS(SELECT 1 FROM ai_screening_cohorts c WHERE c.project_id=project AND c.status='auditing')
 AND NOT EXISTS(SELECT 1 FROM review_run_manifests r
 JOIN ai_screening_cohort_members m ON m.project_id=r.project_id AND m.report_id::text=r.subject->>'report_id'
 JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE r.project_id=project AND r.automation_run_id=run AND c.status IN ('open','closed','auditing','passed'))
$$;
