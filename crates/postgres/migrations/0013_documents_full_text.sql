-- PR8 documents and full-text screening. Evolve the 0006 tables in place.

ALTER TABLE documents
  ADD COLUMN IF NOT EXISTS project_id uuid,
  ADD COLUMN IF NOT EXISTS source text NOT NULL DEFAULT 'upload',
  ADD COLUMN IF NOT EXISTS status text NOT NULL DEFAULT 'uploaded',
  ADD COLUMN IF NOT EXISTS original_filename text,
  ADD COLUMN IF NOT EXISTS external_url text,
  ADD COLUMN IF NOT EXISTS parser_error text,
  ADD COLUMN IF NOT EXISTS active_parser_version text,
  ADD COLUMN IF NOT EXISTS ocr_required boolean NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS actor_kind text NOT NULL DEFAULT 'system',
  ADD COLUMN IF NOT EXISTS actor_id text NOT NULL DEFAULT 'legacy-migration',
  ADD COLUMN IF NOT EXISTS content_available_at timestamptz,
  ADD COLUMN IF NOT EXISTS parsed_at timestamptz,
  ADD COLUMN IF NOT EXISTS failed_at timestamptz,
  ADD COLUMN IF NOT EXISTS updated_at timestamptz NOT NULL DEFAULT now();

DO $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM documents d JOIN project_reports pr ON pr.report_id = d.report_id
    GROUP BY d.id HAVING count(*) > 1
  ) THEN
    RAISE EXCEPTION 'cannot assign legacy documents shared by multiple projects';
  END IF;
END $$;

UPDATE documents d SET project_id = pr.project_id
FROM project_reports pr
WHERE pr.report_id = d.report_id AND d.project_id IS NULL;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM documents WHERE project_id IS NULL) THEN
    RAISE EXCEPTION 'cannot assign a project to every legacy document';
  END IF;
END $$;

UPDATE documents
SET status = CASE parse_status
      WHEN 'pending' THEN 'uploaded'
      WHEN 'parsed' THEN CASE WHEN parser_version IS NULL THEN 'failed' ELSE 'available' END
      WHEN 'ocr_required' THEN CASE WHEN parser_version IS NULL THEN 'failed' ELSE 'available' END
      WHEN 'failed' THEN 'failed' ELSE 'uploaded' END,
    active_parser_version = CASE
      WHEN parse_status IN ('parsed', 'ocr_required') THEN parser_version ELSE NULL END,
    ocr_required = parse_status = 'ocr_required',
    parser_error = CASE
      WHEN parse_status IN ('parsed', 'ocr_required') AND parser_version IS NULL
        THEN 'legacy parser output had no parser version'
      ELSE parser_error END,
    content_available_at = CASE WHEN object_key IS NOT NULL THEN created_at ELSE NULL END,
    parsed_at = CASE WHEN parse_status IN ('parsed', 'ocr_required') THEN created_at ELSE NULL END,
    failed_at = CASE WHEN parse_status = 'failed' THEN created_at ELSE NULL END,
    updated_at = greatest(created_at, updated_at);

ALTER TABLE documents
  ALTER COLUMN project_id SET NOT NULL,
  ALTER COLUMN object_key DROP NOT NULL,
  ALTER COLUMN content_hash DROP NOT NULL,
  DROP CONSTRAINT IF EXISTS documents_content_hash_key,
  DROP CONSTRAINT IF EXISTS documents_parse_status_check,
  DROP CONSTRAINT IF EXISTS documents_source_check,
  DROP CONSTRAINT IF EXISTS documents_status_check,
  DROP CONSTRAINT IF EXISTS documents_actor_kind_check,
  DROP CONSTRAINT IF EXISTS documents_project_report_fk,
  DROP CONSTRAINT IF EXISTS documents_content_shape_check,
  DROP CONSTRAINT IF EXISTS documents_external_url_check,
  DROP CONSTRAINT IF EXISTS documents_object_key_check,
  DROP CONSTRAINT IF EXISTS documents_content_hash_check,
  DROP CONSTRAINT IF EXISTS documents_filename_check,
  DROP CONSTRAINT IF EXISTS documents_parser_shape_check;

ALTER TABLE documents
  ADD CONSTRAINT documents_project_report_fk FOREIGN KEY (project_id, report_id)
    REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE,
  ADD CONSTRAINT documents_source_check CHECK (source IN ('upload', 'external_url', 'resolver')),
  ADD CONSTRAINT documents_status_check CHECK
    (status IN ('missing', 'external', 'uploaded', 'retrieving', 'available', 'failed')),
  ADD CONSTRAINT documents_actor_kind_check CHECK (actor_kind IN ('user', 'automation', 'system')),
  ADD CONSTRAINT documents_actor_id_check CHECK (length(btrim(actor_id)) > 0),
  ADD CONSTRAINT documents_filename_check CHECK
    (original_filename IS NULL OR length(original_filename) BETWEEN 1 AND 255),
  ADD CONSTRAINT documents_external_url_check CHECK (
    (source = 'external_url' AND external_url ~ '^https://[^[:space:]]+$')
    OR (source <> 'external_url' AND external_url IS NULL)
  ),
  ADD CONSTRAINT documents_object_key_check CHECK
    (object_key IS NULL OR object_key ~ '^documents/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'),
  ADD CONSTRAINT documents_content_hash_check CHECK
    (content_hash IS NULL OR content_hash ~ '^[0-9a-f]{64}$'),
  ADD CONSTRAINT documents_content_shape_check CHECK (
    (status IN ('missing', 'external', 'retrieving')
      AND object_key IS NULL AND content_hash IS NULL AND byte_size = 0)
    OR (status IN ('uploaded', 'available')
      AND object_key IS NOT NULL AND content_hash IS NOT NULL
      AND byte_size > 0 AND mime_type = 'application/pdf')
    OR (status = 'failed' AND (
      (object_key IS NULL AND content_hash IS NULL AND byte_size = 0)
      OR (object_key IS NOT NULL AND content_hash IS NOT NULL
        AND byte_size > 0 AND mime_type = 'application/pdf')
    ))
  ),
  ADD CONSTRAINT documents_parser_shape_check CHECK (
    status <> 'available' OR active_parser_version IS NOT NULL
  );

ALTER TABLE documents DROP COLUMN parse_status;

DROP INDEX IF EXISTS documents_project_content_hash_uq;
CREATE UNIQUE INDEX documents_report_content_hash_uq
  ON documents(project_id, report_id, content_hash) WHERE content_hash IS NOT NULL;
CREATE UNIQUE INDEX documents_object_key_uq
  ON documents(object_key) WHERE object_key IS NOT NULL;
CREATE UNIQUE INDEX documents_report_external_url_uq
  ON documents(project_id, report_id, external_url) WHERE external_url IS NOT NULL;
CREATE INDEX documents_project_report_status_idx
  ON documents(project_id, report_id, status, updated_at DESC, id);
CREATE INDEX documents_report_status_idx ON documents(report_id, status, created_at DESC);

ALTER TABLE document_blocks
  ADD COLUMN IF NOT EXISTS active boolean NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS page_width double precision,
  ADD COLUMN IF NOT EXISTS page_height double precision;

UPDATE document_blocks b SET active = d.active_parser_version = b.parser_version
FROM documents d WHERE d.id = b.document_id;

CREATE TABLE document_pages (
  document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  parser_version text NOT NULL,
  page_number integer NOT NULL,
  width double precision NOT NULL,
  height double precision NOT NULL,
  ocr_required boolean NOT NULL DEFAULT false,
  active boolean NOT NULL DEFAULT false,
  PRIMARY KEY(document_id, parser_version, page_number),
  CHECK (length(btrim(parser_version)) > 0),
  CHECK (page_number > 0),
  CHECK (width > 0 AND height > 0
    AND width NOT IN ('Infinity'::float8, '-Infinity'::float8, 'NaN'::float8)
    AND height NOT IN ('Infinity'::float8, '-Infinity'::float8, 'NaN'::float8))
);

INSERT INTO document_pages(document_id, parser_version, page_number, width, height, ocr_required, active)
SELECT b.document_id, b.parser_version, b.page_number,
       coalesce(max(b.page_width), 1), coalesce(max(b.page_height), 1),
       d.ocr_required, bool_or(b.active)
FROM document_blocks b
JOIN documents d ON d.id = b.document_id
GROUP BY b.document_id, b.parser_version, b.page_number, d.ocr_required
ON CONFLICT DO NOTHING;

CREATE UNIQUE INDEX document_pages_active_page_uq
  ON document_pages(document_id, page_number) WHERE active;
CREATE INDEX document_pages_document_version_idx
  ON document_pages(document_id, parser_version, page_number);

ALTER TABLE document_blocks
  ADD CONSTRAINT document_blocks_page_fk
  FOREIGN KEY(document_id, parser_version, page_number)
  REFERENCES document_pages(document_id, parser_version, page_number) ON DELETE CASCADE;

ALTER TABLE document_blocks
  DROP CONSTRAINT IF EXISTS document_blocks_page_number_check,
  DROP CONSTRAINT IF EXISTS document_blocks_ordinal_check,
  DROP CONSTRAINT IF EXISTS document_blocks_kind_check,
  DROP CONSTRAINT IF EXISTS document_blocks_parser_version_check,
  DROP CONSTRAINT IF EXISTS document_blocks_text_check,
  DROP CONSTRAINT IF EXISTS document_blocks_content_hash_check,
  DROP CONSTRAINT IF EXISTS document_blocks_bbox_check,
  DROP CONSTRAINT IF EXISTS document_blocks_page_geometry_check;

ALTER TABLE document_blocks
  ADD CONSTRAINT document_blocks_page_number_check CHECK (page_number > 0),
  ADD CONSTRAINT document_blocks_ordinal_check CHECK (ordinal >= 0),
  ADD CONSTRAINT document_blocks_kind_check CHECK
    (kind IN ('text', 'heading', 'table', 'figure_caption', 'reference')),
  ADD CONSTRAINT document_blocks_parser_version_check CHECK (length(btrim(parser_version)) > 0),
  ADD CONSTRAINT document_blocks_text_check CHECK (length(text) <= 1000000),
  ADD CONSTRAINT document_blocks_content_hash_check CHECK (content_hash ~ '^[0-9a-f]{64}$'),
  ADD CONSTRAINT document_blocks_page_geometry_check CHECK (
    (page_width IS NULL AND page_height IS NULL)
    OR (page_width > 0 AND page_height > 0
      AND page_width NOT IN ('Infinity'::float8, '-Infinity'::float8, 'NaN'::float8)
      AND page_height NOT IN ('Infinity'::float8, '-Infinity'::float8, 'NaN'::float8))
  ),
  ADD CONSTRAINT document_blocks_bbox_check CHECK (
    bbox IS NULL OR (
      jsonb_typeof(bbox) = 'object'
      AND jsonb_typeof(bbox->'x') = 'number' AND jsonb_typeof(bbox->'y') = 'number'
      AND jsonb_typeof(bbox->'width') = 'number' AND jsonb_typeof(bbox->'height') = 'number'
      AND (bbox->>'x')::double precision BETWEEN 0 AND 1
      AND (bbox->>'y')::double precision BETWEEN 0 AND 1
      AND (bbox->>'width')::double precision > 0
      AND (bbox->>'width')::double precision <= 1
      AND (bbox->>'height')::double precision > 0
      AND (bbox->>'height')::double precision <= 1
      AND (bbox->>'x')::double precision + (bbox->>'width')::double precision <= 1
      AND (bbox->>'y')::double precision + (bbox->>'height')::double precision <= 1
    )
  );

CREATE UNIQUE INDEX document_blocks_active_ordinal_uq
  ON document_blocks(document_id, ordinal) WHERE active;
CREATE INDEX document_blocks_document_version_page_idx
  ON document_blocks(document_id, parser_version, page_number, ordinal);
CREATE INDEX IF NOT EXISTS document_blocks_fts_idx ON document_blocks USING gin(search_vector);

ALTER TABLE exclusion_reasons
  ADD CONSTRAINT exclusion_reasons_project_id_id_stage_uq UNIQUE(project_id, id, stage);

ALTER TABLE screening_events
  DROP CONSTRAINT IF EXISTS screening_events_exclusion_reason_id_fkey,
  DROP CONSTRAINT IF EXISTS screening_events_previous_full_text_exclusion_reason_id_fkey,
  DROP CONSTRAINT IF EXISTS screening_events_result_full_text_exclusion_reason_id_fkey,
  DROP CONSTRAINT IF EXISTS screening_events_reason_project_fk,
  DROP CONSTRAINT IF EXISTS screening_events_previous_reason_project_fk,
  DROP CONSTRAINT IF EXISTS screening_events_result_reason_project_fk,
  ADD COLUMN IF NOT EXISTS full_text_reason_stage text GENERATED ALWAYS AS ('full_text') STORED,
  ADD CONSTRAINT screening_events_reason_project_fk FOREIGN KEY(project_id, exclusion_reason_id, stage)
    REFERENCES exclusion_reasons(project_id, id, stage),
  ADD CONSTRAINT screening_events_previous_reason_project_fk
    FOREIGN KEY(project_id, previous_full_text_exclusion_reason_id, full_text_reason_stage)
    REFERENCES exclusion_reasons(project_id, id, stage),
  ADD CONSTRAINT screening_events_result_reason_project_fk
    FOREIGN KEY(project_id, result_full_text_exclusion_reason_id, full_text_reason_stage)
    REFERENCES exclusion_reasons(project_id, id, stage);

ALTER TABLE screening_state
  DROP CONSTRAINT IF EXISTS screening_state_full_text_exclusion_reason_id_fkey,
  DROP CONSTRAINT IF EXISTS screening_state_reason_project_fk,
  ADD COLUMN IF NOT EXISTS full_text_reason_stage text GENERATED ALWAYS AS ('full_text') STORED,
  ADD CONSTRAINT screening_state_reason_project_fk
    FOREIGN KEY(project_id, full_text_exclusion_reason_id, full_text_reason_stage)
    REFERENCES exclusion_reasons(project_id, id, stage);

CREATE OR REPLACE FUNCTION deepref_seed_full_text_reasons(project uuid)
RETURNS void LANGUAGE sql AS $$
  UPDATE exclusion_reasons
  SET code = 'wrong_comparator_outcome', label = 'Wrong comparator/outcome', stage = 'full_text'
  WHERE project_id = project AND code = 'wrong_outcome';
  UPDATE exclusion_reasons
  SET code = 'insufficient_information', label = 'Insufficient information', stage = 'full_text'
  WHERE project_id = project AND code = 'no_usable_full_text';
  INSERT INTO exclusion_reasons(id, project_id, code, label, stage)
  SELECT gen_random_uuid(), project, reason.code, reason.label, 'full_text'
  FROM (VALUES
    ('wrong_population', 'Wrong population'),
    ('wrong_intervention', 'Wrong intervention or exposure'),
    ('wrong_comparator_outcome', 'Wrong comparator/outcome'),
    ('wrong_design', 'Wrong study design'),
    ('conference_abstract_only', 'Conference abstract only'),
    ('duplicate_overlapping_dataset', 'Duplicate or overlapping dataset'),
    ('insufficient_information', 'Insufficient information'),
    ('other', 'Other')
  ) AS reason(code, label)
  ON CONFLICT(project_id, code) DO UPDATE SET label = EXCLUDED.label, stage = 'full_text';
$$;

SELECT deepref_seed_full_text_reasons(id) FROM projects;

CREATE OR REPLACE FUNCTION deepref_seed_full_text_reasons_on_project()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  PERFORM deepref_seed_full_text_reasons(NEW.id);
  RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS projects_seed_full_text_reasons ON projects;
CREATE TRIGGER projects_seed_full_text_reasons AFTER INSERT ON projects
FOR EACH ROW EXECUTE FUNCTION deepref_seed_full_text_reasons_on_project();

CREATE INDEX exclusion_reasons_project_stage_idx
  ON exclusion_reasons(project_id, stage, code);
CREATE INDEX screening_state_title_status_report_idx
  ON screening_state(project_id, title_abstract_status, report_id);
