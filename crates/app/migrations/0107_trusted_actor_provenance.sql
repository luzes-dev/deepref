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

-- N-1 may change a text actor without knowing about its new provenance field.
-- Clear stale provenance whenever a label changes but its UserId does not.
CREATE FUNCTION clear_stale_actor_user_provenance() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
  pair_index integer;
  actor_column text;
  user_column text;
  old_row jsonb := to_jsonb(OLD);
  new_row jsonb := to_jsonb(NEW);
BEGIN
  IF TG_NARGS % 2 <> 0 THEN
    RAISE EXCEPTION 'actor provenance trigger requires actor/provenance column pairs';
  END IF;

  FOR pair_index IN 0..((TG_NARGS / 2) - 1) LOOP
    actor_column := TG_ARGV[pair_index * 2];
    user_column := TG_ARGV[pair_index * 2 + 1];
    IF new_row -> actor_column IS DISTINCT FROM old_row -> actor_column
       AND new_row -> user_column IS NOT DISTINCT FROM old_row -> user_column THEN
      NEW := jsonb_populate_record(NEW, jsonb_build_object(user_column, NULL));
      new_row := to_jsonb(NEW);
    END IF;
  END LOOP;
  RETURN NEW;
END $$;

CREATE TRIGGER screening_events_clear_stale_actor_provenance
BEFORE UPDATE OF actor_id, actor_user_id ON screening_events
FOR EACH ROW EXECUTE FUNCTION clear_stale_actor_user_provenance('actor_id', 'actor_user_id');

CREATE TRIGGER ai_opinion_exposures_clear_stale_actor_provenance
BEFORE UPDATE OF audience_actor_id, audience_actor_user_id ON ai_opinion_exposures
FOR EACH ROW EXECUTE FUNCTION clear_stale_actor_user_provenance('audience_actor_id', 'audience_actor_user_id');

CREATE TRIGGER ai_screening_audit_labels_clear_stale_actor_provenance
BEFORE UPDATE OF actor_id, actor_user_id ON ai_screening_audit_labels
FOR EACH ROW EXECUTE FUNCTION clear_stale_actor_user_provenance('actor_id', 'actor_user_id');

CREATE TRIGGER project_ai_screening_authority_clear_stale_actor_provenance
BEFORE UPDATE OF approved_by, approved_by_actor_user_id ON project_ai_screening_authority
FOR EACH ROW EXECUTE FUNCTION clear_stale_actor_user_provenance('approved_by', 'approved_by_actor_user_id');

CREATE TRIGGER ai_screening_cohorts_clear_stale_actor_provenance
BEFORE UPDATE OF approved_by, approved_by_actor_user_id, finalized_by, finalized_by_actor_user_id
ON ai_screening_cohorts
FOR EACH ROW EXECUTE FUNCTION clear_stale_actor_user_provenance(
  'approved_by', 'approved_by_actor_user_id',
  'finalized_by', 'finalized_by_actor_user_id'
);

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

-- Trusted provenance identifies the actor directly. Exact mappings resolve
-- only historical rows that have no provenance.
CREATE OR REPLACE FUNCTION resolved_human_actor_user_id(
  p_actor_kind text,
  p_actor_id text,
  p_actor_user_id uuid
) RETURNS uuid
LANGUAGE sql STABLE
AS $$
  SELECT CASE WHEN p_actor_kind = 'user'
    THEN COALESCE(
      p_actor_user_id,
      legacy_human_actor_user_id(p_actor_kind, p_actor_id)
    )
  END
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

-- True when the row resolves to U, or when its unresolved legacy label could
-- be U and no operator non-identity decision rules that user out. Trusted
-- provenance takes precedence; exact mappings resolve only legacy rows.
CREATE OR REPLACE FUNCTION actor_identity_may_be_user(
  p_actor_kind text,
  p_actor_id text,
  p_actor_user_id uuid,
  p_user_id uuid
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH identity AS (
    SELECT resolved_human_actor_user_id(
      p_actor_kind, p_actor_id, p_actor_user_id
    ) AS actor_user_id
  )
  SELECT COALESCE(p_actor_kind = 'user'
    AND (
      i.actor_user_id = p_user_id
      OR (
        i.actor_user_id IS NULL
        AND legacy_human_actor_is_unmapped(p_actor_kind, p_actor_id)
        AND NOT EXISTS (
          SELECT 1 FROM legacy_actor_non_identities n
          WHERE n.legacy_actor_kind = p_actor_kind
            AND n.legacy_actor_id = p_actor_id
            AND n.user_id = p_user_id
        )
      )
    ), false)
  FROM identity i
$$;

-- Pairwise distinctness requires two resolved UserIds. Trusted provenance
-- wins over a mapping; a negative decision cannot prove that two unresolved
-- labels belong to different people outside the users table.
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
  WITH identities AS (
    SELECT resolved_human_actor_user_id(
             p_left_kind, p_left_id, p_left_user_id
           ) AS left_user_id,
           resolved_human_actor_user_id(
             p_right_kind, p_right_id, p_right_user_id
           ) AS right_user_id
  )
  SELECT COALESCE(p_left_kind = 'user' AND p_right_kind = 'user'
    AND EXISTS (
      SELECT 1 FROM identities i
      WHERE i.left_user_id IS NULL OR i.right_user_id IS NULL
         OR i.left_user_id = i.right_user_id
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

-- N-1 approval and evaluation code counts audit-label rows. Reject a label
-- before storage unless every other row in its cohort/report has a provably
-- different, resolved identity.
CREATE FUNCTION guard_ai_first_audit_label_identity() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  PERFORM pg_advisory_xact_lock_shared(73003203);
  IF EXISTS (
    SELECT 1 FROM ai_screening_audit_labels l
    WHERE l.cohort_id = NEW.cohort_id AND l.report_id = NEW.report_id
      AND l.id IS DISTINCT FROM CASE WHEN TG_OP = 'UPDATE' THEN OLD.id ELSE NULL END
      AND same_human_actor_identity(
        'user', NEW.actor_id, NEW.actor_user_id,
        'user', l.actor_id, l.actor_user_id
      )
  ) THEN
    RAISE EXCEPTION 'audit label is not provably distinct from an existing reviewer'
      USING ERRCODE = 'check_violation';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER ai_first_audit_label_identity_guard
BEFORE INSERT OR UPDATE OF actor_id, actor_user_id ON ai_screening_audit_labels
FOR EACH ROW EXECUTE FUNCTION guard_ai_first_audit_label_identity();

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
      WHERE same_human_actor_identity(
        'user', p_user_id::text, p_user_id,
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

-- Audit labels affect only that report's reviewer eligibility. The same person
-- can review other blind records independently; an unresolved label blocks all
-- candidate reviewers for this report until it resolves to a different user.
CREATE FUNCTION ai_first_fresh_auditor_v3(p_cohort uuid, p_report uuid, p_user_id uuid)
RETURNS boolean
LANGUAGE sql VOLATILE
AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  )
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
          AND same_human_actor_identity(
            'user', p_user_id::text, p_user_id,
            'user', l.actor_id, l.actor_user_id
          )
      )
  )
  FROM identity_lock
$$;

CREATE FUNCTION ai_first_queue_visible_v3(p_project uuid, p_report uuid, p_user_id uuid)
RETURNS boolean
LANGUAGE sql VOLATILE
AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  )
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
  FROM identity_lock
$$;

CREATE FUNCTION ai_first_audit_has_two_distinct_reviewers(p_cohort uuid, p_report uuid)
RETURNS boolean
LANGUAGE sql VOLATILE
AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  ), labels AS (
    SELECT resolved_human_actor_user_id(
             'user', l.actor_id, l.actor_user_id
           ) AS user_id
    FROM ai_screening_audit_labels l
    WHERE l.cohort_id = p_cohort AND l.report_id = p_report
  )
  SELECT count(*) = 2
    AND count(user_id) = 2
    AND count(DISTINCT user_id) = 2
  FROM labels CROSS JOIN identity_lock
$$;

-- N-1 evaluates and finalizes in application code that still counts rows. Keep
-- the database transition boundary on the same mapping lock and reject legacy
-- evidence that no longer has two resolved, independent reviewers.
CREATE FUNCTION guard_ai_first_identity_transition() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.status IN ('passed', 'finalized') AND NEW.status IS DISTINCT FROM OLD.status THEN
    PERFORM pg_advisory_xact_lock_shared(73003203);
    IF EXISTS (
      SELECT 1 FROM ai_screening_cohort_members m
      WHERE m.cohort_id = NEW.id AND (m.sampled OR m.audit_control)
        AND NOT ai_first_audit_has_two_distinct_reviewers(m.cohort_id, m.report_id)
    ) THEN
      RAISE EXCEPTION 'two resolved, distinct reviewers are required for every audit record'
        USING ERRCODE = 'check_violation';
    END IF;
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER ai_first_identity_transition_guard
BEFORE UPDATE OF status ON ai_screening_cohorts
FOR EACH ROW EXECUTE FUNCTION guard_ai_first_identity_transition();

-- N-1 freshness has no report argument, so it treats every audit label in the
-- cohort as prior history. This is stricter than N's report-scoped rule, but
-- prevents an old direct write from repeating an identity on any report.
-- Unknown labels are compared conservatively against canonical provenance and
-- operator mappings through the same identity helper used by N.
CREATE OR REPLACE FUNCTION ai_first_fresh_auditor(cohort uuid, actor text)
RETURNS boolean
LANGUAGE sql VOLATILE
AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  ), scope AS (
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
  FROM identity_lock
$$;

CREATE OR REPLACE FUNCTION ai_first_queue_visible(project uuid, report uuid, actor text)
RETURNS boolean
LANGUAGE sql VOLATILE
AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  )
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
  FROM identity_lock
$$;

-- The old N caller cannot supply a report to its v2 freshness function. Keep
-- that signature fail-closed so a stale N binary cannot bypass the report-
-- scoped label comparison; upgraded N callers use the three-argument v3 API.
CREATE OR REPLACE FUNCTION ai_first_fresh_auditor_v2(p_cohort uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT false $$;

CREATE OR REPLACE FUNCTION ai_first_queue_visible_v2(p_project uuid, p_report uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT false $$;

-- Mapping and non-identity decisions may report finalized cohorts whose
-- historical reviewer evidence no longer passes the current identity rules.
-- Keep the new array optional so previously deployed N writers still satisfy
-- the same audit shape while the upgraded command records its impact.
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_check;
ALTER TABLE admin_audit_events ADD CONSTRAINT admin_audit_events_check CHECK (
  -- Identity events (0085), unchanged and never workspace-scoped.
  (event_type IN ('auth.login', 'auth.logout', 'user.provisioned')
   AND metadata = '{}'::jsonb
   AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'auth.session_revoked'
      AND metadata ? 'reason'
      AND metadata - 'reason' = '{}'::jsonb
      AND metadata->>'reason' IN ('user_logout', 'user_deactivated')
      AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'user.deactivated'
      AND metadata = '{"source":"provider_event"}'::jsonb
      AND workspace_id IS NULL AND project_id IS NULL)
  -- Workspace lifecycle. `workspace_kind` is `personal` or `organization`;
  -- the migration workspace is created by a migration, not by a request.
  OR (event_type = 'workspace.created'
      AND metadata ? 'workspace_kind'
      AND metadata - 'workspace_kind' = '{}'::jsonb
      AND metadata->>'workspace_kind' IN ('personal', 'organization')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  OR (event_type IN ('workspace.renamed', 'workspace.deleted')
      AND metadata = '{}'::jsonb
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  -- Membership changes name the member in `target_user_id`.
  OR (event_type IN ('membership.created', 'membership.removed')
      AND metadata ? 'role' AND metadata ? 'grant_source'
      AND metadata - 'role' - 'grant_source' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND metadata->>'grant_source' IN ('workos_mirrored', 'local')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  OR (event_type = 'membership.role_changed'
      AND metadata ? 'role'
      AND metadata - 'role' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  OR (event_type = 'membership.status_changed'
      AND metadata ? 'status'
      AND metadata - 'status' = '{}'::jsonb
      AND metadata->>'status' IN ('active', 'deactivated')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  -- Project-access changes name the member and the project.
  OR (event_type IN ('project_access.granted', 'project_access.removed')
      AND metadata ? 'role' AND metadata ? 'grant_source'
      AND metadata - 'role' - 'grant_source' = '{}'::jsonb
      AND metadata->>'role' IN ('project_admin', 'member', 'viewer')
      AND metadata->>'grant_source' IN ('workos_mirrored', 'local')
      AND workspace_id IS NOT NULL AND project_id IS NOT NULL AND target_user_id IS NOT NULL)
  OR (event_type = 'project_access.role_changed'
      AND metadata ? 'role'
      AND metadata - 'role' = '{}'::jsonb
      AND metadata->>'role' IN ('project_admin', 'member', 'viewer')
      AND workspace_id IS NOT NULL AND project_id IS NOT NULL AND target_user_id IS NOT NULL)
  -- Invitations (03a). Never the invitee's email: the row names the workspace
  -- and the role only. Acceptance names the accepting user; the other events
  -- have no user to name (an invitee may not have an account yet).
  OR (event_type IN ('invitation.created', 'invitation.revoked', 'invitation.expired')
      AND metadata ? 'role' AND metadata ? 'invitation_id'
      AND metadata - 'role' - 'invitation_id' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND metadata->>'invitation_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  OR (event_type = 'invitation.accepted'
      AND metadata ? 'role' AND metadata ? 'invitation_id'
      AND metadata - 'role' - 'invitation_id' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND metadata->>'invitation_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  -- Authorization denials (03b). High volume and caller-influenced, so the
  -- row is a fixed shape: the permission and resource type come from closed
  -- lists, `window` is the minute bucket (epoch minutes), and the resource
  -- identifier is stored only when it parsed as a UUID. No request body, path
  -- text or caller-supplied string is ever stored. 0100 makes one row per
  -- (user, permission, resource type, window) the most the table can hold.
  OR (event_type = 'authorization.denied'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL
      AND target_user_id IS NULL
      AND metadata ? 'permission' AND metadata ? 'resource_type' AND metadata ? 'window'
      AND metadata - 'permission' - 'resource_type' - 'window' = '{}'::jsonb
      AND metadata->>'permission' IN (
        'workspace:view', 'workspace:manage', 'workspace:billing', 'workspace:members:invite',
        'project:create', 'project:view', 'project:edit', 'project:delete',
        'screening:perform', 'screening:resolve', 'extraction:perform', 'appraisal:perform',
        'automation:manage', 'ai:use', 'api_key:manage',
        'route_unlisted', 'platform_operator')
      AND metadata->>'resource_type' IN ('project', 'workspace', 'none')
      AND metadata->>'window' ~ '^[0-9]{1,10}$'
      AND ((metadata->>'resource_type' = 'project' AND workspace_id IS NULL)
        OR (metadata->>'resource_type' = 'workspace' AND project_id IS NULL)
        OR (metadata->>'resource_type' = 'none' AND workspace_id IS NULL AND project_id IS NULL)))
  -- Legacy identity decisions (03c). The old label is retained in the audited metadata;
  -- only trusted operator commands can create these rows.
  OR (event_type IN ('legacy_actor.mapped', 'legacy_actor.non_identity_recorded')
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL
      AND target_user_id IS NOT NULL
      AND workspace_id IS NULL AND project_id IS NULL
      AND metadata ? 'legacy_actor_kind' AND metadata ? 'legacy_actor_id' AND metadata ? 'source'
      AND (NOT (metadata ? 'affected_finalized_cohorts')
        OR jsonb_typeof(metadata->'affected_finalized_cohorts') = 'array')
      AND metadata - 'legacy_actor_kind' - 'legacy_actor_id' - 'source'
        - 'affected_finalized_cohorts' = '{}'::jsonb
      AND metadata->>'legacy_actor_kind' = 'user'
      AND btrim(metadata->>'legacy_actor_id') <> ''
      AND length(metadata->>'legacy_actor_id') <= 200
      AND btrim(metadata->>'source') <> ''
      AND length(metadata->>'source') <= 200)
) NOT VALID;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
