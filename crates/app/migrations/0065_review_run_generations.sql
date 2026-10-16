-- A failed or blocked review run must not reserve its subject forever (review
-- ai F1). The manifest hash stays the identity of the work; a generation number
-- separates attempts at it. Generation 0 keeps the historical idempotency key
-- `review:{manifest_hash}`; generation n uses `review:{manifest_hash}:{n}`.
-- A regeneration opens a new reuse epoch, so no step attempt or model run from
-- an earlier epoch is reused for a fresh opinion.
ALTER TABLE review_run_manifests
  ADD COLUMN generation integer NOT NULL DEFAULT 0,
  ADD COLUMN reuse_epoch integer NOT NULL DEFAULT 0,
  ADD CONSTRAINT review_run_manifests_generation_check
    CHECK (generation >= 0 AND reuse_epoch >= 0 AND reuse_epoch <= generation);
CREATE UNIQUE INDEX review_run_manifests_generation_idx
  ON review_run_manifests(project_id, manifest_hash, generation);

-- The automatic second reviewer claimed one attempt per record, stage and
-- protocol forever. Each attempt now remembers its run, so a failed or blocked
-- run can be retried after a backoff, a bounded number of times.
ALTER TABLE second_review_attempts
  ADD COLUMN review_run_id uuid,
  ADD COLUMN attempts integer NOT NULL DEFAULT 1 CHECK (attempts >= 1),
  ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();

-- Link existing attempts to the advisory run they scheduled, newest first.
UPDATE second_review_attempts a
SET review_run_id = (
  SELECT m.automation_run_id FROM review_run_manifests m
  WHERE m.project_id = a.project_id
    AND m.origin->>'kind' = 'advisory_triggered'
    AND m.subject->>'report_id' = a.report_id::text
    AND m.subject->>'stage' = a.stage
    AND m.subject->>'protocol_version_id' = a.protocol_version_id::text
  ORDER BY m.created_at DESC, m.automation_run_id DESC
  LIMIT 1
);
