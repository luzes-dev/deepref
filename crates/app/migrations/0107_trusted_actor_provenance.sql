-- phase: expand
-- Canonical human identity is recorded as trusted UserId provenance. Existing
-- text actor labels stay untouched and all new provenance columns are nullable
-- so N-1 writers remain compatible during rollout.
ALTER TABLE screening_events
  ADD COLUMN actor_user_id uuid REFERENCES users(id);

ALTER TABLE ai_opinion_exposures
  ADD COLUMN audience_actor_user_id uuid REFERENCES users(id);

ALTER TABLE ai_screening_audit_labels
  ADD COLUMN actor_user_id uuid REFERENCES users(id);

ALTER TABLE project_ai_screening_authority
  ADD COLUMN approved_by_actor_user_id uuid REFERENCES users(id);

ALTER TABLE ai_screening_cohorts
  ADD COLUMN approved_by_actor_user_id uuid REFERENCES users(id),
  ADD COLUMN finalized_by_actor_user_id uuid REFERENCES users(id);

-- Finalizer provenance changes with the existing finalization fields after a
-- frozen design reaches its terminal state. The original approval provenance
-- remains part of the frozen evidence and cannot be rewritten.
CREATE OR REPLACE FUNCTION protect_ai_first_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
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
   (to_jsonb(NEW)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by',
                        'finalized_by_actor_user_id','finalized_at','invalidation_reason',
                        'identity_flagged_at','identity_flag_reason'])
   IS DISTINCT FROM
   (to_jsonb(OLD)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by',
                        'finalized_by_actor_user_id','finalized_at','invalidation_reason',
                        'identity_flagged_at','identity_flag_reason']) THEN
   RAISE EXCEPTION 'sample design is frozen';
 END IF;
 RETURN NEW;
END $$;

ALTER TABLE screening_events
  ADD CONSTRAINT screening_events_actor_user_id_kind_check
  CHECK (actor_user_id IS NULL OR actor_kind = 'user') NOT VALID;

ALTER TABLE ai_opinion_exposures
  ADD CONSTRAINT ai_opinion_exposures_audience_user_id_shape_check
  CHECK (audience_actor_user_id IS NULL OR
    (audience_actor_kind = 'user' AND audience_actor_id IS NOT NULL)) NOT VALID;

-- N writes use one row per authenticated person. N-1 rows keep a NULL user ID
-- and are checked against exact, operator-confirmed labels below.
CREATE UNIQUE INDEX ai_screening_audit_labels_actor_user_id_uq
  ON ai_screening_audit_labels (cohort_id, report_id, actor_user_id)
  WHERE actor_user_id IS NOT NULL;

-- UUID text is never resolved by parsing its shape. Only an exact operator
-- mapping can resolve a legacy text label to a user.
CREATE OR REPLACE FUNCTION legacy_human_actor_matches_user(
  p_actor_kind text,
  p_actor_id text,
  p_user_id uuid
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT p_actor_kind = 'user'
    AND EXISTS (
      SELECT 1 FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
        AND m.user_id = p_user_id
    )
$$;

CREATE OR REPLACE FUNCTION legacy_human_actor_user_id(
  p_actor_kind text,
  p_actor_id text
) RETURNS uuid
LANGUAGE sql STABLE
AS $$
  SELECT m.user_id
  FROM legacy_actor_mappings m
  WHERE p_actor_kind = 'user'
    AND m.legacy_actor_kind = p_actor_kind
    AND m.legacy_actor_id = p_actor_id
$$;

CREATE OR REPLACE FUNCTION legacy_human_actor_is_unmapped(
  p_actor_kind text,
  p_actor_id text
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT p_actor_kind = 'user'
    AND p_actor_id IS NOT NULL
    AND NOT EXISTS (
      SELECT 1 FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
    )
$$;

-- True when the row is U, or when its unresolved legacy label could be U and
-- no operator non-identity decision rules that user out. Provenance and an
-- exact mapping are both consulted; an unresolved UUID-shaped label is still
-- unresolved.
CREATE OR REPLACE FUNCTION actor_identity_may_be_user(
  p_actor_kind text,
  p_actor_id text,
  p_actor_user_id uuid,
  p_user_id uuid
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT COALESCE(p_actor_kind = 'user'
    AND (
      p_actor_user_id = p_user_id
      OR legacy_human_actor_matches_user(p_actor_kind, p_actor_id, p_user_id)
      OR (
        p_actor_user_id IS NULL
        AND legacy_human_actor_is_unmapped(p_actor_kind, p_actor_id)
        AND NOT EXISTS (
          SELECT 1 FROM legacy_actor_non_identities n
          WHERE n.legacy_actor_kind = p_actor_kind
            AND n.legacy_actor_id = p_actor_id
            AND n.user_id = p_user_id
        )
      )
    ), false)
$$;

-- Provenance-aware comparison used when two historical identity inputs are
-- compared. Unknown labels conservatively intersect each known identity unless
-- an operator recorded that label as a non-identity for that user.
CREATE FUNCTION same_human_actor_identity(
  p_left_kind text,
  p_left_id text,
  p_left_user_id uuid,
  p_right_kind text,
  p_right_id text,
  p_right_user_id uuid
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH resolved AS (
    SELECT legacy_human_actor_user_id(p_left_kind, p_left_id) AS left_mapped_user_id,
           legacy_human_actor_user_id(p_right_kind, p_right_id) AS right_mapped_user_id
  ), candidates AS (
    SELECT p_left_user_id AS left_user_id,
           r.left_mapped_user_id,
           p_right_user_id AS right_user_id,
           r.right_mapped_user_id
    FROM resolved r
  )
  SELECT COALESCE(p_left_kind = 'user' AND p_right_kind = 'user'
    AND (
      p_left_id = p_right_id
      OR EXISTS (
        SELECT 1 FROM candidates c
        WHERE c.left_user_id IS NOT NULL
          AND c.left_user_id IN (c.right_user_id, c.right_mapped_user_id)
      )
      OR EXISTS (
        SELECT 1 FROM candidates c
        WHERE c.left_mapped_user_id IS NOT NULL
          AND c.left_mapped_user_id IN (c.right_user_id, c.right_mapped_user_id)
      )
      OR EXISTS (
        SELECT 1 FROM candidates c
        WHERE p_left_user_id IS NULL
          AND c.left_mapped_user_id IS NULL
          AND legacy_human_actor_is_unmapped(p_left_kind, p_left_id)
          AND (c.right_user_id IS NOT NULL OR c.right_mapped_user_id IS NOT NULL)
          AND NOT EXISTS (
            SELECT 1 FROM legacy_actor_non_identities n
            WHERE n.legacy_actor_kind = p_left_kind
              AND n.legacy_actor_id = p_left_id
              AND n.user_id IN (c.right_user_id, c.right_mapped_user_id)
          )
      )
      OR EXISTS (
        SELECT 1 FROM candidates c
        WHERE p_right_user_id IS NULL
          AND c.right_mapped_user_id IS NULL
          AND legacy_human_actor_is_unmapped(p_right_kind, p_right_id)
          AND (c.left_user_id IS NOT NULL OR c.left_mapped_user_id IS NOT NULL)
          AND NOT EXISTS (
            SELECT 1 FROM legacy_actor_non_identities n
            WHERE n.legacy_actor_kind = p_right_kind
              AND n.legacy_actor_id = p_right_id
              AND n.user_id IN (c.left_user_id, c.left_mapped_user_id)
          )
      )
      OR EXISTS (
        SELECT 1 FROM candidates c
        WHERE p_left_user_id IS NULL
          AND c.left_mapped_user_id IS NULL
          AND p_right_user_id IS NULL
          AND c.right_mapped_user_id IS NULL
          AND legacy_human_actor_is_unmapped(p_left_kind, p_left_id)
          AND legacy_human_actor_is_unmapped(p_right_kind, p_right_id)
          AND EXISTS (
            SELECT 1 FROM users u
            WHERE NOT EXISTS (
              SELECT 1 FROM legacy_actor_non_identities n
              WHERE n.user_id = u.id
                AND ((n.legacy_actor_kind = p_left_kind AND n.legacy_actor_id = p_left_id)
                  OR (n.legacy_actor_kind = p_right_kind AND n.legacy_actor_id = p_right_id))
            )
          )
      )
    ), false)
$$;

-- Preserve the old four-text comparison for existing callers, but make it
-- mapping-aware and conservative. New callers pass provenance explicitly.
CREATE OR REPLACE FUNCTION same_human_actor_identity(
  p_left_kind text,
  p_left_id text,
  p_right_kind text,
  p_right_id text
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT same_human_actor_identity(
    p_left_kind, p_left_id, NULL::uuid,
    p_right_kind, p_right_id, NULL::uuid
  )
$$;

CREATE FUNCTION ai_first_human_history_clear(p_cohort uuid, p_user_id uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH scope AS (
    SELECT c.id, c.project_id,
           c.approved_by, c.approved_by_actor_user_id,
           c.finalized_by, c.finalized_by_actor_user_id
    FROM ai_screening_cohorts c WHERE c.id = p_cohort
  ), history AS (
    SELECT 'user'::text AS actor_kind, s.approved_by AS actor_id,
           s.approved_by_actor_user_id AS actor_user_id FROM scope s
    UNION ALL
    SELECT 'user'::text, s.finalized_by, s.finalized_by_actor_user_id
    FROM scope s WHERE s.finalized_by IS NOT NULL
    UNION ALL
    SELECT 'user'::text, a.approved_by, a.approved_by_actor_user_id
    FROM scope s JOIN project_ai_screening_authority a ON a.project_id = s.project_id
    UNION ALL
    SELECT e.actor_kind, e.actor_id, e.actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN screening_events e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.actor_kind = 'user'
    UNION ALL
    SELECT e.audience_actor_kind, e.audience_actor_id, e.audience_actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN ai_opinion_exposures e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.audience_actor_kind = 'user' AND e.audience_actor_id IS NOT NULL
  )
  SELECT EXISTS (
    SELECT 1 FROM scope s
    WHERE NOT EXISTS (
      SELECT 1 FROM history h
      WHERE actor_identity_may_be_user(
        h.actor_kind, h.actor_id, h.actor_user_id, p_user_id
      )
    )
      AND NOT EXISTS (
        SELECT 1 FROM ai_screening_cohort_members m
        JOIN ai_opinion_exposures e
          ON e.project_id = m.project_id AND e.report_id = m.report_id
        WHERE m.cohort_id = s.id AND e.audience_actor_id IS NULL
      )
  )
$$;

-- Audit labels affect only that report's reviewer eligibility. The same person
-- can review other blind records independently; an unresolved label blocks U
-- on this report until it is mapped to someone else or denied for U.
CREATE FUNCTION ai_first_fresh_auditor_v3(p_cohort uuid, p_report uuid, p_user_id uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT EXISTS (
    SELECT 1 FROM ai_screening_cohorts c
    WHERE c.id = p_cohort
      AND ai_first_human_history_clear(c.id, p_user_id)
      AND EXISTS (
        SELECT 1 FROM scientific_assignments a
        WHERE a.project_id = c.project_id
          AND a.user_id = p_user_id
          AND a.role = 'fresh_ai_auditor'
          AND a.revoked_at IS NULL
      )
      AND NOT EXISTS (
        SELECT 1 FROM ai_screening_audit_labels l
        WHERE l.cohort_id = c.id AND l.report_id = p_report
          AND actor_identity_may_be_user('user', l.actor_id, l.actor_user_id, p_user_id)
      )
  )
$$;

CREATE FUNCTION ai_first_queue_visible_v3(p_project uuid, p_report uuid, p_user_id uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT CASE
    WHEN ai_first_audit_masked(p_project, p_report) THEN EXISTS (
      SELECT 1 FROM ai_screening_cohort_members m
      JOIN ai_screening_cohorts c ON c.id = m.cohort_id
      WHERE m.project_id = p_project AND m.report_id = p_report
        AND c.status = 'auditing' AND (m.sampled OR m.audit_control)
        AND ai_first_fresh_auditor_v3(c.id, p_report, p_user_id)
        AND (SELECT count(*) FROM ai_screening_audit_labels l
             WHERE l.cohort_id = c.id AND l.report_id = p_report) < 2
    )
    ELSE NOT EXISTS (
      SELECT 1 FROM ai_screening_dispositions d
      JOIN ai_screening_cohorts c ON c.id = d.cohort_id
      WHERE d.project_id = p_project AND d.report_id = p_report
        AND d.voided_at IS NULL AND d.finalized_at IS NULL
        AND c.status IN ('open', 'closed', 'passed')
    )
  END
$$;

CREATE FUNCTION ai_first_audit_has_two_distinct_reviewers(p_cohort uuid, p_report uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH labels AS (
    SELECT l.actor_user_id,
           legacy_human_actor_user_id('user', l.actor_id) AS mapped_user_id
    FROM ai_screening_audit_labels l
    WHERE l.cohort_id = p_cohort AND l.report_id = p_report
  ), resolved AS (
    SELECT actor_user_id, mapped_user_id,
           COALESCE(actor_user_id, mapped_user_id) AS user_id
    FROM labels
  )
  SELECT count(*) = 2
    AND count(user_id) = 2
    AND count(DISTINCT user_id) = 2
    AND bool_and(actor_user_id IS NULL OR mapped_user_id IS NULL OR actor_user_id = mapped_user_id)
  FROM resolved
$$;

-- N-1 freshness has no report argument, so it treats every audit label in the
-- cohort as prior history. This is stricter than N's report-scoped rule, but
-- prevents an old direct write from repeating an identity on any report.
-- Unknown labels are compared conservatively against canonical provenance and
-- operator mappings through the same identity helper used by N.
CREATE OR REPLACE FUNCTION ai_first_fresh_auditor(cohort uuid, actor text)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH scope AS (
    SELECT c.id, c.project_id,
           c.approved_by, c.approved_by_actor_user_id,
           c.finalized_by, c.finalized_by_actor_user_id
    FROM ai_screening_cohorts c WHERE c.id = cohort
  ), history AS (
    SELECT 'user'::text AS actor_kind, s.approved_by AS actor_id,
           s.approved_by_actor_user_id AS actor_user_id FROM scope s
    UNION ALL
    SELECT 'user'::text, s.finalized_by, s.finalized_by_actor_user_id
    FROM scope s WHERE s.finalized_by IS NOT NULL
    UNION ALL
    SELECT 'user'::text, a.approved_by, a.approved_by_actor_user_id
    FROM scope s JOIN project_ai_screening_authority a ON a.project_id = s.project_id
    UNION ALL
    SELECT e.actor_kind, e.actor_id, e.actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN screening_events e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.actor_kind = 'user'
    UNION ALL
    SELECT e.audience_actor_kind, e.audience_actor_id, e.audience_actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN ai_opinion_exposures e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.audience_actor_id IS NOT NULL
    UNION ALL
    SELECT 'user'::text, l.actor_id, l.actor_user_id
    FROM scope s JOIN ai_screening_audit_labels l ON l.cohort_id = s.id
  )
  SELECT actor IS NOT NULL AND EXISTS (
    SELECT 1 FROM scope s
    WHERE NOT EXISTS (
      SELECT 1 FROM history h
      WHERE same_human_actor_identity(
        'user', actor, NULL::uuid,
        h.actor_kind, h.actor_id, h.actor_user_id
      )
    )
      AND NOT EXISTS (
        SELECT 1 FROM ai_screening_cohort_members m
        JOIN ai_opinion_exposures e
          ON e.project_id = m.project_id AND e.report_id = m.report_id
        WHERE m.cohort_id = s.id AND e.audience_actor_id IS NULL
      )
  )
$$;

CREATE OR REPLACE FUNCTION ai_first_queue_visible(project uuid, report uuid, actor text)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT CASE
    WHEN ai_first_audit_masked(project, report) THEN EXISTS (
      SELECT 1 FROM ai_screening_cohort_members m
      JOIN ai_screening_cohorts c ON c.id = m.cohort_id
      WHERE m.project_id = project AND m.report_id = report
        AND c.status = 'auditing' AND (m.sampled OR m.audit_control)
        AND ai_first_fresh_auditor(c.id, actor)
        AND NOT EXISTS (
          SELECT 1 FROM ai_screening_audit_labels l
          WHERE l.cohort_id = c.id AND l.report_id = report
            AND same_human_actor_identity(
              'user', actor, NULL::uuid,
              'user', l.actor_id, l.actor_user_id
            )
        )
        AND (SELECT count(*) FROM ai_screening_audit_labels l
             WHERE l.cohort_id = c.id AND l.report_id = report) < 2
    )
    ELSE NOT EXISTS (
      SELECT 1 FROM ai_screening_dispositions d
      JOIN ai_screening_cohorts c ON c.id = d.cohort_id
      WHERE d.project_id = project AND d.report_id = report
        AND d.voided_at IS NULL AND d.finalized_at IS NULL
        AND c.status IN ('open', 'closed', 'passed')
    )
  END
$$;

-- The old N caller cannot supply a report to its v2 freshness function. Keep
-- that signature fail-closed so a stale N binary cannot bypass the report-
-- scoped label comparison; upgraded N callers use the three-argument v3 API.
CREATE OR REPLACE FUNCTION ai_first_fresh_auditor_v2(p_cohort uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT false $$;

CREATE OR REPLACE FUNCTION ai_first_queue_visible_v2(p_project uuid, p_report uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT false $$;
