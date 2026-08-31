-- PR12 typed AI screening and duplicate-assistance proposal projections.
-- AI remains advisory: these columns and joins describe a proposal and never
-- replace screening_state, screening_events, records, or project_reports.

CREATE UNIQUE INDEX IF NOT EXISTS ai_proposals_project_id_id_uq
  ON ai_proposals(project_id, id);
CREATE UNIQUE INDEX IF NOT EXISTS eligibility_criteria_protocol_id_uq
  ON eligibility_criteria(protocol_version_id, id);
CREATE UNIQUE INDEX IF NOT EXISTS documents_project_report_id_uq
  ON documents(project_id, report_id, id);
CREATE UNIQUE INDEX IF NOT EXISTS document_blocks_document_id_id_uq
  ON document_blocks(document_id, id);

ALTER TABLE ai_proposals
  ADD COLUMN IF NOT EXISTS task_kind text,
  ADD COLUMN IF NOT EXISTS target_report_id uuid,
  ADD COLUMN IF NOT EXISTS target_record_id uuid,
  ADD COLUMN IF NOT EXISTS protocol_version_id uuid,
  ADD COLUMN IF NOT EXISTS expected_revision bigint;

UPDATE ai_proposals p
SET task_kind = COALESCE(NULLIF(p.task_kind, ''), r.task_kind)
FROM ai_runs r
WHERE r.id = p.model_run_id AND (p.task_kind IS NULL OR p.task_kind = '');

UPDATE ai_proposals
SET task_kind = COALESCE(NULLIF(task_kind, ''), proposal_type)
WHERE task_kind IS NULL OR task_kind = '';

ALTER TABLE ai_proposals
  ALTER COLUMN task_kind SET NOT NULL,
  ADD CONSTRAINT ai_proposals_task_kind_check CHECK (length(btrim(task_kind)) > 0),
  ADD CONSTRAINT ai_proposals_expected_revision_check
    CHECK (expected_revision IS NULL OR expected_revision >= 0),
  ADD CONSTRAINT ai_proposals_project_report_target_fkey
    FOREIGN KEY (project_id, target_report_id)
    REFERENCES project_reports(project_id, report_id) ON DELETE RESTRICT,
  ADD CONSTRAINT ai_proposals_project_record_target_fkey
    FOREIGN KEY (project_id, target_record_id)
    REFERENCES records(project_id, id) ON DELETE RESTRICT,
  ADD CONSTRAINT ai_proposals_project_protocol_target_fkey
    FOREIGN KEY (project_id, protocol_version_id)
    REFERENCES protocol_versions(project_id, id) ON DELETE RESTRICT;

CREATE UNIQUE INDEX IF NOT EXISTS ai_proposals_project_id_id_protocol_uq
  ON ai_proposals(project_id, id, protocol_version_id);

CREATE INDEX IF NOT EXISTS ai_proposals_project_task_status_idx
  ON ai_proposals(project_id, task_kind, status, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS ai_proposals_project_target_idx
  ON ai_proposals(project_id, target_report_id, target_record_id, created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS ai_proposal_criterion_judgments (
  proposal_id uuid NOT NULL,
  project_id uuid NOT NULL,
  criterion_id uuid NOT NULL,
  protocol_version_id uuid NOT NULL,
  ordinal integer NOT NULL CHECK (ordinal >= 0),
  judgment text NOT NULL CHECK (judgment IN ('meets', 'does_not_meet', 'unclear')),
  rationale text NOT NULL CHECK (length(btrim(rationale)) > 0),
  evidence jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence) = 'array'),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (proposal_id, criterion_id),
  UNIQUE (proposal_id, ordinal),
  CONSTRAINT ai_proposal_criterion_judgments_proposal_project_fkey
    FOREIGN KEY (proposal_id, project_id) REFERENCES ai_proposals(id, project_id)
    ON DELETE CASCADE,
  CONSTRAINT ai_proposal_criterion_judgments_proposal_protocol_fkey
    FOREIGN KEY (proposal_id, project_id, protocol_version_id)
    REFERENCES ai_proposals(id, project_id, protocol_version_id)
    ON DELETE CASCADE,
  CONSTRAINT ai_proposal_criterion_judgments_protocol_version_id_fkey
    FOREIGN KEY (project_id, protocol_version_id)
    REFERENCES protocol_versions(project_id, id)
    ON DELETE RESTRICT,
  CONSTRAINT ai_proposal_criterion_judgments_criterion_fkey
    FOREIGN KEY (protocol_version_id, criterion_id)
    REFERENCES eligibility_criteria(protocol_version_id, id)
    ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS ai_proposal_criterion_project_idx
  ON ai_proposal_criterion_judgments(project_id, proposal_id, ordinal);

CREATE TABLE IF NOT EXISTS ai_proposal_evidence (
  proposal_id uuid NOT NULL,
  project_id uuid NOT NULL,
  ordinal integer NOT NULL CHECK (ordinal >= 0),
  evidence_kind text NOT NULL CHECK (evidence_kind IN ('report_metadata', 'document_block')),
  report_id uuid NOT NULL,
  document_id uuid,
  document_block_id uuid,
  page integer CHECK (page IS NULL OR page > 0),
  source_field text CHECK (source_field IS NULL OR source_field IN ('title', 'abstract')),
  content_hash text NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (proposal_id, ordinal),
  CONSTRAINT ai_proposal_evidence_proposal_project_fkey
    FOREIGN KEY (proposal_id, project_id) REFERENCES ai_proposals(id, project_id)
    ON DELETE CASCADE,
  CONSTRAINT ai_proposal_evidence_project_report_fkey
    FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id)
    ON DELETE RESTRICT,
  CONSTRAINT ai_proposal_evidence_document_project_report_fkey
    FOREIGN KEY (project_id, report_id, document_id)
    REFERENCES documents(project_id, report_id, id) ON DELETE RESTRICT,
  CONSTRAINT ai_proposal_evidence_document_block_fkey
    FOREIGN KEY (document_id, document_block_id)
    REFERENCES document_blocks(document_id, id) ON DELETE RESTRICT,
  CHECK (
    (evidence_kind = 'report_metadata'
      AND source_field IS NOT NULL
      AND document_id IS NULL
      AND document_block_id IS NULL
      AND page IS NULL)
    OR
    (evidence_kind = 'document_block'
      AND source_field IS NULL
      AND document_id IS NOT NULL
      AND document_block_id IS NOT NULL
      AND page IS NOT NULL)
  )
);

CREATE INDEX IF NOT EXISTS ai_proposal_evidence_project_idx
  ON ai_proposal_evidence(project_id, proposal_id, ordinal);
