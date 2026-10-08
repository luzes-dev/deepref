-- PubMed ID imports: a list of PMIDs is one acquisition run with one item per PMID, so the
-- run inspector can say what happened to each. Items wait as 'queued' until the worker
-- fetches them from PubMed; record_id links the record an import saved.
CREATE TABLE IF NOT EXISTS pmid_import_items (
  acquisition_run_id uuid NOT NULL REFERENCES acquisition_runs(id) ON DELETE CASCADE,
  pmid text NOT NULL CHECK (pmid ~ '^[1-9][0-9]{0,8}$'),
  position integer NOT NULL,
  status text NOT NULL CHECK (status IN ('queued', 'imported', 'already_in_project', 'not_found', 'failed')),
  title text,
  record_id uuid REFERENCES records(id) ON DELETE SET NULL,
  last_error text,
  queued_at timestamptz NOT NULL DEFAULT now(),
  processed_at timestamptz,
  PRIMARY KEY (acquisition_run_id, pmid),
  UNIQUE (acquisition_run_id, position)
);

CREATE INDEX IF NOT EXISTS pmid_import_items_queued_idx
  ON pmid_import_items (acquisition_run_id, position)
  WHERE status = 'queued';
