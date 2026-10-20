-- phase: expand
-- Identity maps are the only authority for resolving legacy text. The v2
-- functions fail closed until trusted actor provenance is available in 0107.

CREATE FUNCTION legacy_human_actor_matches_user(
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

CREATE FUNCTION legacy_human_actor_user_id(
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

CREATE FUNCTION legacy_human_actor_is_unmapped(
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

-- These signatures do not identify the target report. They cannot safely
-- account for report-scoped prior labels, so they remain fail-closed. N uses
-- the provenance-aware v3 functions created by migration 0107.
CREATE FUNCTION ai_first_fresh_auditor_v2(p_cohort uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT false $$;

CREATE FUNCTION ai_first_queue_visible_v2(p_project uuid, p_report uuid, p_user_id uuid)
RETURNS boolean LANGUAGE sql STABLE AS $$ SELECT false $$;
