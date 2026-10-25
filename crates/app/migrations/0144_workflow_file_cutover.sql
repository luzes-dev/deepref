-- phase: expand
-- An ordinary upgrade preserves every released compatibility byte and NOT NULL.
CREATE TABLE workflow_file_storage_state (
  singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
  mode text NOT NULL DEFAULT 'overlap' CHECK(mode IN ('overlap','object_only','contracted')),
  changed_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO workflow_file_storage_state(singleton) VALUES(true);

-- Prevent an old writer from silently undoing the verified backfill after
-- explicit drain/cutover. Old binaries must be drained, not merely sampled.
CREATE FUNCTION workflow_file_storage_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_mode text;
BEGIN
  SELECT mode INTO STRICT current_mode FROM workflow_file_storage_state WHERE singleton FOR SHARE;
  IF current_mode <> 'overlap' AND (current_setting('deepref.workflow_file_writer', true) IS DISTINCT FROM 'object-v1'
     OR NEW.storage_key IS NULL OR NEW.content_sha256 IS NULL OR NEW.byte_size IS NULL) THEN
    RAISE EXCEPTION 'workflow file object-only cutover rejects legacy writer';
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER workflow_file_storage_guard_trigger BEFORE INSERT OR UPDATE ON workflow_files
  FOR EACH ROW EXECUTE FUNCTION workflow_file_storage_guard();
