-- Does this PDF belong to the report it is attached to? After each parse the worker compares
-- the PDF's title and DOI with the report's and stores the verdict. A mismatch is a flag for the
-- researcher, not a block: they keep the PDF (identity_acknowledged_at) or remove it.
ALTER TABLE documents
  ADD COLUMN IF NOT EXISTS identity_check jsonb,
  ADD COLUMN IF NOT EXISTS identity_acknowledged_at timestamptz;
