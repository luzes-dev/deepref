-- Reviewer-entered extraction values.  Without an AI provider the sheet would
-- stay empty forever, so a reviewer may record a value directly.  Such a value
-- may cite a source block but does not have to; the source group is therefore
-- all-or-nothing.  A new value supersedes the previous one instead of
-- replacing it, which keeps the full history of who recorded what.

ALTER TABLE extraction_values
  ALTER COLUMN report_id DROP NOT NULL,
  ALTER COLUMN source_document_id DROP NOT NULL,
  ALTER COLUMN source_block_id DROP NOT NULL,
  ALTER COLUMN source_page DROP NOT NULL,
  ALTER COLUMN source_parser_version DROP NOT NULL,
  ALTER COLUMN source_content_hash DROP NOT NULL,
  ALTER COLUMN rationale DROP NOT NULL,
  ADD COLUMN superseded_at timestamptz,
  ADD COLUMN superseded_by_actor_kind text
    CHECK (superseded_by_actor_kind IN ('user', 'automation', 'system')),
  ADD COLUMN superseded_by_actor_id text
    CHECK (length(btrim(superseded_by_actor_id)) > 0),
  ADD CONSTRAINT extraction_values_source_all_or_none CHECK (
    (report_id IS NULL AND source_document_id IS NULL AND source_block_id IS NULL
       AND source_page IS NULL AND source_parser_version IS NULL AND source_content_hash IS NULL)
    OR (report_id IS NOT NULL AND source_document_id IS NOT NULL AND source_block_id IS NOT NULL
       AND source_page IS NOT NULL AND source_parser_version IS NOT NULL
       AND source_content_hash IS NOT NULL)
  ),
  ADD CONSTRAINT extraction_values_supersession_actor CHECK (
    (superseded_at IS NULL AND superseded_by_actor_kind IS NULL AND superseded_by_actor_id IS NULL)
    OR (superseded_at IS NOT NULL AND superseded_by_actor_kind IS NOT NULL
       AND superseded_by_actor_id IS NOT NULL)
  );

DO $$
DECLARE
  unique_constraint text;
BEGIN
  SELECT conname INTO unique_constraint
  FROM pg_constraint
  WHERE conrelid = 'extraction_values'::regclass AND contype = 'u';
  IF unique_constraint IS NOT NULL THEN
    EXECUTE format('ALTER TABLE extraction_values DROP CONSTRAINT %I', unique_constraint);
  END IF;
END $$;

CREATE UNIQUE INDEX extraction_values_current_uq
  ON extraction_values(project_id, study_id, field_definition_id, field_definition_version)
  WHERE superseded_at IS NULL;
