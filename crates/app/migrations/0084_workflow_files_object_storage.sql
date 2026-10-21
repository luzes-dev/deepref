-- Expand step of moving workflow_files bodies out of PostgreSQL into object
-- storage. The three reference columns are nullable: a row without a storage
-- key still carries its bytes in `content`, which stays NOT NULL and keeps
-- being written until old readers have drained. Writers set the key, hash and
-- size together with `content`; the backfill job sets them on older rows.
ALTER TABLE workflow_files
  ADD COLUMN storage_key text,
  ADD COLUMN content_sha256 text,
  ADD COLUMN byte_size bigint;

-- Validated separately so the table is only scanned under a lock that lets
-- reads and writes continue.
ALTER TABLE workflow_files
  ADD CONSTRAINT workflow_files_object_reference_check CHECK (
    (storage_key IS NULL AND content_sha256 IS NULL AND byte_size IS NULL)
    OR (
      storage_key IS NOT NULL
      AND content_sha256 ~ '^[0-9a-f]{64}$'
      AND byte_size >= 0
    )
  ) NOT VALID;
ALTER TABLE workflow_files VALIDATE CONSTRAINT workflow_files_object_reference_check;

-- Rows the backfill job still has to move.
CREATE INDEX workflow_files_unmigrated_idx
  ON workflow_files (project_id, id)
  WHERE storage_key IS NULL;
