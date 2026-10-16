-- PR6: versioned, project-scoped review protocols.
-- Existing 0006 protocol rows remain readable. The legacy `criteria` JSON column
-- is retained as a compatibility projection; the ordered criteria table below is
-- authoritative for new writes.

ALTER TABLE protocol_versions
  ADD COLUMN IF NOT EXISTS framework_kind text NOT NULL DEFAULT 'custom',
  ADD COLUMN IF NOT EXISTS framework_fields jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN IF NOT EXISTS objective text NOT NULL DEFAULT '',
  ADD COLUMN IF NOT EXISTS question text NOT NULL DEFAULT '',
  ADD COLUMN IF NOT EXISTS revision bigint NOT NULL DEFAULT 1,
  ADD COLUMN IF NOT EXISTS amendment_of uuid,
  ADD COLUMN IF NOT EXISTS updated_at timestamptz NOT NULL DEFAULT now(),
  ADD COLUMN IF NOT EXISTS created_by_kind text NOT NULL DEFAULT 'system',
  ADD COLUMN IF NOT EXISTS created_by_id text NOT NULL DEFAULT 'migration',
  ADD COLUMN IF NOT EXISTS updated_by_kind text NOT NULL DEFAULT 'system',
  ADD COLUMN IF NOT EXISTS updated_by_id text NOT NULL DEFAULT 'migration',
  ADD COLUMN IF NOT EXISTS published_by_kind text,
  ADD COLUMN IF NOT EXISTS published_by_id text;

ALTER TABLE protocol_versions
  ADD CONSTRAINT protocol_versions_framework_kind_check
    CHECK (framework_kind IN ('pico', 'picos', 'peco', 'peo', 'pcc', 'spider', 'custom')),
  ADD CONSTRAINT protocol_versions_revision_check CHECK (revision >= 1),
  ADD CONSTRAINT protocol_versions_version_positive_check CHECK (version >= 1),
  ADD CONSTRAINT protocol_versions_amendment_not_self_check
    CHECK (amendment_of IS NULL OR amendment_of <> id),
  ADD CONSTRAINT protocol_versions_project_id_id_unique UNIQUE (project_id, id),
  ADD CONSTRAINT protocol_versions_amendment_project_fkey
    FOREIGN KEY (project_id, amendment_of)
    REFERENCES protocol_versions (project_id, id)
    ON DELETE CASCADE;

ALTER TABLE screening_events
  DROP CONSTRAINT IF EXISTS screening_events_protocol_version_id_fkey;

ALTER TABLE screening_events
  ADD CONSTRAINT screening_events_project_protocol_fkey
    FOREIGN KEY (project_id, protocol_version_id)
    REFERENCES protocol_versions (project_id, id)
    ON DELETE CASCADE;

-- These partial unique indexes encode the legal aggregate states. A project may
-- have many historical superseded versions, but only one editable draft and one
-- current published version.
CREATE UNIQUE INDEX protocol_versions_one_draft_per_project_idx
  ON protocol_versions (project_id)
  WHERE status = 'draft';

CREATE UNIQUE INDEX protocol_versions_one_current_published_per_project_idx
  ON protocol_versions (project_id)
  WHERE status = 'published';

ALTER TABLE eligibility_criteria
  ADD COLUMN IF NOT EXISTS stage text NOT NULL DEFAULT 'both',
  ADD COLUMN IF NOT EXISTS dimension text NOT NULL DEFAULT 'other';

ALTER TABLE eligibility_criteria
  ADD CONSTRAINT eligibility_criteria_stage_check
    CHECK (stage IN ('title_abstract', 'full_text', 'both')),
  ADD CONSTRAINT eligibility_criteria_dimension_check
    CHECK (dimension IN (
      'population', 'intervention', 'comparator', 'outcome', 'design',
      'setting', 'language', 'date', 'other'
    ));

-- Migration 0006 seeded the compatibility JSON column but did not populate the
-- ordered criteria table. Derive stable UUIDs from the protocol and ordinal so
-- this backfill is deterministic and safe to run again during development.
INSERT INTO eligibility_criteria (
  id, protocol_version_id, criterion_type, stage, dimension,
  label, description, ordinal
)
SELECT
  format('%s-%s-%s-%s-%s',
    substr(md5('deepref:protocol-criterion:' || pv.id::text || ':' || (item.ordinal - 1)::text), 1, 8),
    substr(md5('deepref:protocol-criterion:' || pv.id::text || ':' || (item.ordinal - 1)::text), 9, 4),
    substr(md5('deepref:protocol-criterion:' || pv.id::text || ':' || (item.ordinal - 1)::text), 13, 4),
    substr(md5('deepref:protocol-criterion:' || pv.id::text || ':' || (item.ordinal - 1)::text), 17, 4),
    substr(md5('deepref:protocol-criterion:' || pv.id::text || ':' || (item.ordinal - 1)::text), 21, 12)
  )::uuid,
  pv.id,
  CASE lower(coalesce(item.value->>'kind', item.value->>'criterion_type', 'include'))
    WHEN 'exclude' THEN 'exclude'
    WHEN 'exclusion' THEN 'exclude'
    ELSE 'include'
  END,
  CASE lower(coalesce(item.value->>'stage', 'both'))
    WHEN 'title_abstract' THEN 'title_abstract'
    WHEN 'full_text' THEN 'full_text'
    ELSE 'both'
  END,
  CASE lower(coalesce(item.value->>'dimension', item.value->>'id', 'other'))
    WHEN 'population' THEN 'population'
    WHEN 'intervention' THEN 'intervention'
    WHEN 'comparator' THEN 'comparator'
    WHEN 'outcome' THEN 'outcome'
    WHEN 'design' THEN 'design'
    WHEN 'setting' THEN 'setting'
    WHEN 'language' THEN 'language'
    WHEN 'date' THEN 'date'
    ELSE 'other'
  END,
  coalesce(nullif(btrim(item.value->>'label'), ''), format('Criterion %s', item.ordinal)),
  coalesce(
    nullif(btrim(item.value->>'description'), ''),
    nullif(btrim(item.value->>'label'), ''),
    format('Legacy criterion %s', item.ordinal)
  ),
  (item.ordinal - 1)::integer
FROM protocol_versions pv
CROSS JOIN LATERAL jsonb_array_elements(
  CASE WHEN jsonb_typeof(pv.criteria) = 'array' THEN pv.criteria ELSE '[]'::jsonb END
) WITH ORDINALITY AS item(value, ordinal)
WHERE NOT EXISTS (
  SELECT 1
  FROM eligibility_criteria existing
  WHERE existing.protocol_version_id = pv.id
    AND existing.ordinal = (item.ordinal - 1)::integer
)
ON CONFLICT (id) DO NOTHING;

CREATE INDEX eligibility_criteria_protocol_ordinal_idx
  ON eligibility_criteria (protocol_version_id, ordinal);

COMMENT ON COLUMN protocol_versions.criteria IS
  'Legacy compatibility projection; new protocol writes use eligibility_criteria.';
COMMENT ON COLUMN protocol_versions.framework_fields IS
  'Structured framework fields validated by the domain boundary.';

-- The service only changes a draft scientific aggregate. Published and
-- superseded rows are immutable scientific artifacts; status, audit timestamps
-- and publication provenance are the only allowed lifecycle updates.
CREATE OR REPLACE FUNCTION prevent_published_protocol_content_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    -- Cascading project deletion invokes this trigger at a deeper level. The
    -- project FK owns that cleanup; direct deletion of a historical artifact
    -- remains forbidden.
    IF pg_trigger_depth() > 1 THEN
      RETURN OLD;
    END IF;
    IF OLD.status IN ('published', 'superseded') THEN
      RAISE EXCEPTION 'published protocol scientific content is immutable'
        USING ERRCODE = 'check_violation';
    END IF;
    RETURN OLD;
  END IF;

  IF OLD.status = 'superseded' AND NEW.status <> 'superseded' THEN
    RAISE EXCEPTION 'superseded protocol lifecycle is immutable'
      USING ERRCODE = 'check_violation';
  END IF;
  IF OLD.status = 'published' AND NEW.status NOT IN ('published', 'superseded') THEN
    RAISE EXCEPTION 'published protocol can only become superseded'
      USING ERRCODE = 'check_violation';
  END IF;
  IF OLD.status IN ('published', 'superseded') AND (
    OLD.project_id IS DISTINCT FROM NEW.project_id OR
    OLD.version IS DISTINCT FROM NEW.version OR
    OLD.name IS DISTINCT FROM NEW.name OR
    OLD.criteria IS DISTINCT FROM NEW.criteria OR
    OLD.framework_kind IS DISTINCT FROM NEW.framework_kind OR
    OLD.framework_fields IS DISTINCT FROM NEW.framework_fields OR
    OLD.objective IS DISTINCT FROM NEW.objective OR
    OLD.question IS DISTINCT FROM NEW.question OR
    OLD.amendment_of IS DISTINCT FROM NEW.amendment_of OR
    OLD.revision IS DISTINCT FROM NEW.revision
  ) THEN
    RAISE EXCEPTION 'published protocol scientific content is immutable'
      USING ERRCODE = 'check_violation';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS protocol_versions_immutable_content_trigger ON protocol_versions;
CREATE TRIGGER protocol_versions_immutable_content_trigger
  BEFORE UPDATE OR DELETE ON protocol_versions
  FOR EACH ROW
  EXECUTE FUNCTION prevent_published_protocol_content_mutation();

CREATE OR REPLACE FUNCTION prevent_published_criterion_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  protocol_id uuid;
BEGIN
  -- Criteria are deleted by the protocol/project FK during aggregate cleanup.
  IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
    RETURN OLD;
  END IF;

  IF TG_OP = 'DELETE' THEN
    protocol_id := OLD.protocol_version_id;
  ELSE
    protocol_id := NEW.protocol_version_id;
  END IF;

  IF EXISTS (
    SELECT 1
    FROM protocol_versions
    WHERE id = protocol_id AND status IN ('published', 'superseded')
  ) THEN
    RAISE EXCEPTION 'published protocol criteria are immutable'
      USING ERRCODE = 'check_violation';
  END IF;

  IF TG_OP = 'UPDATE' AND OLD.protocol_version_id IS DISTINCT FROM NEW.protocol_version_id
     AND EXISTS (
       SELECT 1
       FROM protocol_versions
       WHERE id = OLD.protocol_version_id AND status IN ('published', 'superseded')
     ) THEN
    RAISE EXCEPTION 'published protocol criteria are immutable'
      USING ERRCODE = 'check_violation';
  END IF;

  IF TG_OP = 'DELETE' THEN
    RETURN OLD;
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS eligibility_criteria_immutable_trigger ON eligibility_criteria;
CREATE TRIGGER eligibility_criteria_immutable_trigger
  BEFORE INSERT OR UPDATE OR DELETE ON eligibility_criteria
  FOR EACH ROW
  EXECUTE FUNCTION prevent_published_criterion_mutation();
