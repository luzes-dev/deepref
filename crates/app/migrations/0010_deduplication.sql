-- PR5: deterministic source-record deduplication.
--
-- The SQL backfill below is intentionally conservative. PostgreSQL cannot
-- reproduce the Rust NFKC + Unicode-category normalizer byte-for-byte without
-- embedding application code, so the resolver rewrites normalized_title with
-- deepref_domain::normalize_bibliography_title before scoring every legacy
-- record/report it touches. New imports use the Rust normalizer at insertion.

ALTER TABLE reports
  ADD COLUMN IF NOT EXISTS normalized_title text,
  ADD COLUMN IF NOT EXISTS authors jsonb NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE records
  ADD COLUMN IF NOT EXISTS normalized_title text;

-- Seed a useful trigram shortlist for existing rows. This is not the
-- authoritative Unicode normalization; the resolver recomputes touched rows.
UPDATE reports
SET normalized_title = NULLIF(
  btrim(regexp_replace(regexp_replace(lower(title), '[[:punct:]]+', ' ', 'g'), '[[:space:]]+', ' ', 'g')),
  ''
)
WHERE title IS NOT NULL AND normalized_title IS NULL;

UPDATE records
SET normalized_title = NULLIF(
  btrim(regexp_replace(regexp_replace(lower(title), '[[:punct:]]+', ' ', 'g'), '[[:space:]]+', ' ', 'g')),
  ''
)
WHERE title IS NOT NULL AND normalized_title IS NULL;

ALTER TABLE records
  ADD CONSTRAINT records_project_id_id_key UNIQUE (project_id, id);

CREATE INDEX IF NOT EXISTS reports_normalized_title_trgm_idx
  ON reports USING gin (normalized_title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS records_unresolved_project_idx
  ON records (project_id, created_at, id)
  WHERE report_id IS NULL;
CREATE INDEX IF NOT EXISTS project_reports_project_report_idx
  ON project_reports (project_id, report_id);

CREATE TABLE IF NOT EXISTS dedupe_proposals (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  record_id uuid NOT NULL,
  candidate_report_id uuid,
  proposal_kind text NOT NULL CHECK (proposal_kind IN ('fuzzy', 'conflict')),
  title_similarity double precision NOT NULL DEFAULT 0 CHECK (title_similarity >= 0 AND title_similarity <= 1),
  year_match boolean,
  first_author_similarity double precision CHECK (first_author_similarity IS NULL OR (first_author_similarity >= 0 AND first_author_similarity <= 1)),
  exact_identifier_match boolean NOT NULL DEFAULT false,
  conflicting_identifier boolean NOT NULL DEFAULT false,
  score double precision NOT NULL DEFAULT 0 CHECK (score >= 0 AND score <= 1),
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'rejected')),
  revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
  reviewer_kind text,
  reviewer_id text,
  decided_at timestamptz,
  decision_reason text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, id),
  FOREIGN KEY (project_id, record_id) REFERENCES records(project_id, id) ON DELETE CASCADE,
  CHECK (btrim(coalesce(reviewer_kind, '')) = '' OR reviewer_kind IN ('user', 'automation', 'system')),
  CHECK (btrim(coalesce(reviewer_id, '')) = '' OR btrim(reviewer_id) <> ''),
  CONSTRAINT dedupe_proposals_status_reviewer_check CHECK (
    (status = 'pending'
      AND reviewer_kind IS NULL
      AND reviewer_id IS NULL
      AND decided_at IS NULL
      AND decision_reason IS NULL)
    OR
    (status IN ('accepted', 'rejected')
      AND btrim(coalesce(reviewer_kind, '')) <> ''
      AND btrim(coalesce(reviewer_id, '')) <> ''
      AND decided_at IS NOT NULL
      AND btrim(coalesce(decision_reason, '')) <> '')
  )
);

-- Candidate reports are valid only when they are members of the same project.
-- Drop the earlier global-only constraint when upgrading a development database.
ALTER TABLE dedupe_proposals
  DROP CONSTRAINT IF EXISTS dedupe_proposals_candidate_report_id_fkey;
ALTER TABLE dedupe_proposals
  ADD COLUMN IF NOT EXISTS decision_reason text;
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_proposals_candidate_project_report_fkey'
  ) THEN
    ALTER TABLE dedupe_proposals
      ADD CONSTRAINT dedupe_proposals_candidate_project_report_fkey
      FOREIGN KEY (project_id, candidate_report_id)
      REFERENCES project_reports(project_id, report_id)
      ON DELETE CASCADE;
  END IF;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS dedupe_pending_proposal_unique_idx
  ON dedupe_proposals (
    project_id,
    record_id,
    coalesce(candidate_report_id, '00000000-0000-0000-0000-000000000000'::uuid),
    proposal_kind
  )
  WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS dedupe_proposals_project_status_idx
  ON dedupe_proposals (project_id, status, created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS dedupe_resolution_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  record_id uuid NOT NULL,
  prior_report_id uuid,
  resolved_report_id uuid,
  action text NOT NULL CHECK (action IN ('auto_link', 'create_report', 'accept_proposal', 'reject_proposal', 'create_new', 'link', 'reassign', 'revert')),
  reason text NOT NULL CHECK (btrim(reason) <> ''),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (btrim(actor_id) <> ''),
  proposal_id uuid,
  reverted_event_id uuid,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, id),
  UNIQUE (project_id, record_id, id),
  FOREIGN KEY (project_id, record_id) REFERENCES records(project_id, id) ON DELETE CASCADE
);

-- Constraints below make rerunning the migration safe for databases created by
-- the first PR5 draft while preserving the project-scoped audit contract.
ALTER TABLE dedupe_resolution_events
  DROP CONSTRAINT IF EXISTS dedupe_resolution_events_prior_report_id_fkey,
  DROP CONSTRAINT IF EXISTS dedupe_resolution_events_resolved_report_id_fkey,
  DROP CONSTRAINT IF EXISTS dedupe_resolution_events_proposal_id_fkey;
ALTER TABLE dedupe_resolution_events
  ADD COLUMN IF NOT EXISTS reverted_event_id uuid;
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_proposals_project_id_id_key'
  ) THEN
    ALTER TABLE dedupe_proposals
      ADD CONSTRAINT dedupe_proposals_project_id_id_key UNIQUE (project_id, id);
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_proposals_status_reviewer_check'
  ) THEN
    ALTER TABLE dedupe_proposals
      ADD CONSTRAINT dedupe_proposals_status_reviewer_check CHECK (
        (status = 'pending'
          AND reviewer_kind IS NULL
          AND reviewer_id IS NULL
          AND decided_at IS NULL
          AND decision_reason IS NULL)
        OR
        (status IN ('accepted', 'rejected')
          AND btrim(coalesce(reviewer_kind, '')) <> ''
          AND btrim(coalesce(reviewer_id, '')) <> ''
          AND decided_at IS NOT NULL
          AND btrim(coalesce(decision_reason, '')) <> '')
      ) NOT VALID;
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_resolution_events_prior_project_report_fkey'
  ) THEN
    ALTER TABLE dedupe_resolution_events
      ADD CONSTRAINT dedupe_resolution_events_prior_project_report_fkey
      FOREIGN KEY (project_id, prior_report_id)
      REFERENCES project_reports(project_id, report_id) ON DELETE RESTRICT;
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_resolution_events_resolved_project_report_fkey'
  ) THEN
    ALTER TABLE dedupe_resolution_events
      ADD CONSTRAINT dedupe_resolution_events_resolved_project_report_fkey
      FOREIGN KEY (project_id, resolved_report_id)
      REFERENCES project_reports(project_id, report_id) ON DELETE RESTRICT;
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_resolution_events_proposal_project_fkey'
  ) THEN
    ALTER TABLE dedupe_resolution_events
      ADD CONSTRAINT dedupe_resolution_events_proposal_project_fkey
      FOREIGN KEY (project_id, proposal_id)
      REFERENCES dedupe_proposals(project_id, id) ON DELETE RESTRICT;
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'dedupe_resolution_events_reverted_event_fkey'
  ) THEN
    ALTER TABLE dedupe_resolution_events
      ADD CONSTRAINT dedupe_resolution_events_reverted_event_fkey
      FOREIGN KEY (project_id, record_id, reverted_event_id)
      REFERENCES dedupe_resolution_events(project_id, record_id, id) ON DELETE RESTRICT;
  END IF;
END $$;

CREATE INDEX IF NOT EXISTS dedupe_resolution_events_record_idx
  ON dedupe_resolution_events (project_id, record_id, created_at DESC, id DESC);
