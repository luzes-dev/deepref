-- PR13 AI appraisal, study grouping, and durable extraction.
-- AI remains proposal-only.  These constraints make every accepted extraction
-- value prove its project, study, report, document, block, and parser scope.

ALTER TABLE ai_proposals
  ADD COLUMN target_study_id uuid;

ALTER TABLE ai_proposals
  ADD CONSTRAINT ai_proposals_project_study_target_fkey
    FOREIGN KEY (project_id, target_study_id)
    REFERENCES studies(project_id, id) ON DELETE RESTRICT;

CREATE INDEX ai_proposals_project_study_target_idx
  ON ai_proposals(project_id, target_study_id, created_at DESC, id DESC);

CREATE TABLE extraction_field_definitions (
  id uuid NOT NULL,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  version integer NOT NULL CHECK (version > 0),
  field_key text NOT NULL CHECK (length(btrim(field_key)) BETWEEN 1 AND 100),
  label text NOT NULL CHECK (length(btrim(label)) BETWEEN 1 AND 200),
  value_type text NOT NULL CHECK (value_type IN ('text', 'number', 'boolean', 'date')),
  required boolean NOT NULL DEFAULT false,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, id, version),
  UNIQUE (project_id, field_key, version)
);

CREATE INDEX extraction_field_definitions_project_idx
  ON extraction_field_definitions(project_id, field_key, version DESC);

ALTER TABLE document_blocks
  ADD CONSTRAINT document_blocks_document_id_id_parser_uq
    UNIQUE (document_id, id, parser_version);

CREATE TABLE extraction_values (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL,
  study_id uuid NOT NULL,
  report_id uuid NOT NULL,
  field_definition_id uuid NOT NULL,
  field_definition_version integer NOT NULL CHECK (field_definition_version > 0),
  value_type text NOT NULL CHECK (value_type IN ('text', 'number', 'boolean', 'date')),
  text_value text,
  number_value double precision,
  boolean_value boolean,
  date_value date,
  rationale text NOT NULL CHECK (length(btrim(rationale)) > 0),
  source_document_id uuid NOT NULL,
  source_block_id uuid NOT NULL,
  source_page integer NOT NULL CHECK (source_page > 0),
  source_parser_version text NOT NULL CHECK (length(btrim(source_parser_version)) > 0),
  source_content_hash text NOT NULL CHECK (source_content_hash ~ '^[0-9a-f]{64}$'),
  approved_by_actor_kind text NOT NULL CHECK (approved_by_actor_kind IN ('user', 'automation', 'system')),
  approved_by_actor_id text NOT NULL CHECK (length(btrim(approved_by_actor_id)) > 0),
  approved_at timestamptz NOT NULL DEFAULT now(),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, study_id, field_definition_id, field_definition_version),
  -- Membership is checked in the acceptance transaction, not stored as a
  -- foreign key: later study moves/removals must not erase provenance.
  FOREIGN KEY (project_id, study_id)
    REFERENCES studies(project_id, id) ON DELETE RESTRICT,
  FOREIGN KEY (project_id, report_id)
    REFERENCES project_reports(project_id, report_id) ON DELETE RESTRICT,
  FOREIGN KEY (project_id, field_definition_id, field_definition_version)
    REFERENCES extraction_field_definitions(project_id, id, version) ON DELETE RESTRICT,
  FOREIGN KEY (project_id, report_id, source_document_id)
    REFERENCES documents(project_id, report_id, id) ON DELETE RESTRICT,
  FOREIGN KEY (source_document_id, source_block_id, source_parser_version)
    REFERENCES document_blocks(document_id, id, parser_version) ON DELETE RESTRICT,
  CHECK (
    (value_type = 'text' AND text_value IS NOT NULL AND number_value IS NULL AND boolean_value IS NULL AND date_value IS NULL)
    OR (value_type = 'number' AND text_value IS NULL AND number_value IS NOT NULL
      AND number_value = number_value
      AND number_value < 'Infinity'::double precision
      AND number_value > '-Infinity'::double precision
      AND number_value::text NOT IN ('NaN', 'Infinity', '-Infinity')
      AND boolean_value IS NULL AND date_value IS NULL)
    OR (value_type = 'boolean' AND text_value IS NULL AND number_value IS NULL AND boolean_value IS NOT NULL AND date_value IS NULL)
    OR (value_type = 'date' AND text_value IS NULL AND number_value IS NULL AND boolean_value IS NULL AND date_value IS NOT NULL)
  )
);

CREATE INDEX extraction_values_project_study_idx
  ON extraction_values(project_id, study_id, created_at DESC, id DESC);
CREATE INDEX extraction_values_source_idx
  ON extraction_values(project_id, report_id, source_document_id, source_block_id);

CREATE TABLE extraction_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL,
  study_id uuid NOT NULL,
  proposal_id uuid NOT NULL,
  event_type text NOT NULL CHECK (event_type = 'extraction_values_approved'),
  payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (proposal_id),
  FOREIGN KEY (project_id, study_id) REFERENCES studies(project_id, id) ON DELETE RESTRICT,
  FOREIGN KEY (project_id, proposal_id) REFERENCES ai_proposals(project_id, id) ON DELETE RESTRICT
);

CREATE INDEX extraction_events_project_study_idx
  ON extraction_events(project_id, study_id, created_at DESC, id DESC);
