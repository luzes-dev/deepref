-- phase: expand
-- N-1 binaries keep calling ai_first_fresh_auditor and
-- ai_first_queue_visible with their legacy labels. These v2 functions are
-- additive and take the canonical authenticated UserId for N callers.

CREATE FUNCTION legacy_human_actor_matches_user(
  p_actor_kind text,
  p_actor_id text,
  p_user_id uuid
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT p_actor_kind = 'user'
    AND (p_actor_id = p_user_id::text OR EXISTS (
      SELECT 1 FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
        AND m.user_id = p_user_id
    ))
$$;

CREATE FUNCTION legacy_human_actor_user_id(
  p_actor_kind text,
  p_actor_id text
) RETURNS uuid
LANGUAGE sql STABLE
AS $$
  SELECT CASE
    WHEN p_actor_kind = 'user'
      AND p_actor_id ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      THEN p_actor_id::uuid
    ELSE (
      SELECT m.user_id FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
    )
  END
$$;

CREATE FUNCTION legacy_human_actor_is_unmapped(
  p_actor_kind text,
  p_actor_id text
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT p_actor_kind = 'user'
    AND p_actor_id !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    AND NOT EXISTS (
      SELECT 1 FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
    )
$$;

CREATE FUNCTION same_human_actor_identity(
  p_left_kind text,
  p_left_id text,
  p_right_kind text,
  p_right_id text
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH resolved AS (
    SELECT legacy_human_actor_user_id(p_left_kind, p_left_id) AS left_user_id,
           legacy_human_actor_user_id(p_right_kind, p_right_id) AS right_user_id
  )
  SELECT p_left_kind = 'user' AND p_right_kind = 'user'
    AND (
      p_left_id = p_right_id
      OR EXISTS (
        SELECT 1 FROM resolved r
        WHERE r.left_user_id IS NOT NULL AND r.left_user_id = r.right_user_id
      )
      OR EXISTS (
        SELECT 1 FROM resolved r
        WHERE legacy_human_actor_is_unmapped(p_left_kind, p_left_id)
          AND r.right_user_id IS NOT NULL
          AND NOT EXISTS (
            SELECT 1 FROM legacy_actor_non_identities n
            WHERE n.legacy_actor_kind = p_left_kind
              AND n.legacy_actor_id = p_left_id
              AND n.user_id = r.right_user_id
          )
      )
      OR EXISTS (
        SELECT 1 FROM resolved r
        WHERE legacy_human_actor_is_unmapped(p_right_kind, p_right_id)
          AND r.left_user_id IS NOT NULL
          AND NOT EXISTS (
            SELECT 1 FROM legacy_actor_non_identities n
            WHERE n.legacy_actor_kind = p_right_kind
              AND n.legacy_actor_id = p_right_id
              AND n.user_id = r.left_user_id
          )
      )
      OR (
        legacy_human_actor_is_unmapped(p_left_kind, p_left_id)
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
    )
$$;

CREATE FUNCTION ai_first_fresh_auditor_v2(p_cohort uuid, p_user_id uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  WITH scope AS (
    SELECT c.id, c.project_id, c.approved_by, c.finalized_by
    FROM ai_screening_cohorts c
    WHERE c.id = p_cohort
  ), history AS (
    SELECT 'user'::text AS actor_kind, s.approved_by AS actor_id FROM scope s
    UNION ALL
    SELECT 'user'::text, s.finalized_by FROM scope s WHERE s.finalized_by IS NOT NULL
    UNION ALL
    SELECT 'user'::text, a.approved_by
    FROM scope s
    JOIN project_ai_screening_authority a ON a.project_id = s.project_id
    UNION ALL
    SELECT e.actor_kind, e.actor_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN screening_events e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.actor_kind = 'user'
    UNION ALL
    SELECT e.audience_actor_kind, e.audience_actor_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN ai_opinion_exposures e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.audience_actor_kind = 'user' AND e.audience_actor_id IS NOT NULL
  )
  SELECT EXISTS (
    SELECT 1
    FROM scope s
    WHERE EXISTS (
      SELECT 1 FROM scientific_assignments a
      WHERE a.project_id = s.project_id
        AND a.user_id = p_user_id
        AND a.role = 'fresh_ai_auditor'
        AND a.revoked_at IS NULL
    )
      AND NOT EXISTS (
        SELECT 1
        FROM ai_screening_cohort_members m
        JOIN ai_opinion_exposures e
          ON e.project_id = m.project_id AND e.report_id = m.report_id
        WHERE m.cohort_id = s.id AND e.audience_actor_id IS NULL
      )
      AND NOT EXISTS (
        SELECT 1
        FROM history h
        WHERE legacy_human_actor_matches_user(h.actor_kind, h.actor_id, p_user_id)
           OR (
             h.actor_kind = 'user'
             AND h.actor_id !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND NOT EXISTS (
               SELECT 1 FROM legacy_actor_mappings m
               WHERE m.legacy_actor_kind = h.actor_kind AND m.legacy_actor_id = h.actor_id
             )
             AND NOT EXISTS (
               SELECT 1 FROM legacy_actor_non_identities n
               WHERE n.legacy_actor_kind = h.actor_kind
                 AND n.legacy_actor_id = h.actor_id
                 AND n.user_id = p_user_id
             )
           )
      )
  )
$$;

CREATE FUNCTION ai_first_queue_visible_v2(
  p_project uuid,
  p_report uuid,
  p_user_id uuid
) RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT CASE
    WHEN ai_first_audit_masked(p_project, p_report) THEN EXISTS (
      SELECT 1
      FROM ai_screening_cohort_members m
      JOIN ai_screening_cohorts c ON c.id = m.cohort_id
      WHERE m.project_id = p_project AND m.report_id = p_report
        AND c.status = 'auditing'
        AND (m.sampled OR m.audit_control)
        AND ai_first_fresh_auditor_v2(c.id, p_user_id)
        AND NOT EXISTS (
          SELECT 1 FROM ai_screening_audit_labels l
          WHERE l.cohort_id = c.id AND l.report_id = p_report
            AND legacy_human_actor_matches_user('user', l.actor_id, p_user_id)
        )
        AND (
          SELECT count(*) FROM ai_screening_audit_labels l
          WHERE l.cohort_id = c.id AND l.report_id = p_report
        ) < 2
    )
    ELSE NOT EXISTS (
      SELECT 1
      FROM ai_screening_dispositions d
      JOIN ai_screening_cohorts c ON c.id = d.cohort_id
      WHERE d.project_id = p_project AND d.report_id = p_report
        AND d.voided_at IS NULL AND d.finalized_at IS NULL
        AND c.status IN ('open', 'closed', 'passed')
    )
  END
$$;
