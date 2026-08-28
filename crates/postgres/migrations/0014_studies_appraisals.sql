-- PR9 study grouping and generic appraisal. This migration evolves the shallow
-- PR2 study tables without rewriting history or encoding domain transitions in triggers.

ALTER TABLE studies
  ADD COLUMN study_revision bigint NOT NULL DEFAULT 0,
  ADD COLUMN design_context jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN updated_by_actor_kind text NOT NULL DEFAULT 'system',
  ADD COLUMN updated_by_actor_id text NOT NULL DEFAULT 'legacy-migration',
  ADD COLUMN classified_at timestamptz;

UPDATE studies
SET title = COALESCE(NULLIF(left(btrim(title), 200), ''), 'Study ' || id::text),
    design = CASE lower(btrim(design))
  WHEN 'rct' THEN 'rct'
  WHEN 'non_randomized_intervention' THEN 'non_randomized_intervention'
  WHEN 'cohort' THEN 'cohort'
  WHEN 'case_control' THEN 'case_control'
  WHEN 'cross_sectional' THEN 'cross_sectional'
  WHEN 'diagnostic_accuracy' THEN 'diagnostic_accuracy'
  WHEN 'prediction_model' THEN 'prediction_model'
  WHEN 'qualitative' THEN 'qualitative'
  WHEN 'systematic_review' THEN 'systematic_review'
  WHEN 'case_series' THEN 'case_series'
  ELSE NULL
END
;

ALTER TABLE studies
  ALTER COLUMN title SET NOT NULL,
  ADD CONSTRAINT studies_revision_check CHECK (study_revision >= 0),
  ADD CONSTRAINT studies_design_check CHECK (
    design IS NULL OR design IN (
      'rct', 'non_randomized_intervention', 'cohort', 'case_control',
      'cross_sectional', 'diagnostic_accuracy', 'prediction_model',
      'qualitative', 'systematic_review', 'case_series'
    )
  ),
  ADD CONSTRAINT studies_title_check CHECK (
    length(btrim(title)) > 0 AND char_length(title) <= 200
  ),
  ADD CONSTRAINT studies_design_context_object_check CHECK (jsonb_typeof(design_context) = 'object'),
  ADD CONSTRAINT studies_updated_actor_kind_check CHECK
    (updated_by_actor_kind IN ('user', 'automation', 'system')),
  ADD CONSTRAINT studies_updated_actor_id_check CHECK (length(btrim(updated_by_actor_id)) > 0),
  ADD CONSTRAINT studies_project_id_id_uq UNIQUE (project_id, id);

ALTER TABLE study_reports ADD COLUMN project_id uuid;

UPDATE study_reports sr
SET project_id = s.project_id
FROM studies s
WHERE s.id = sr.study_id;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM study_reports WHERE project_id IS NULL) THEN
    RAISE EXCEPTION 'cannot project legacy study membership into a project scope';
  END IF;
END $$;

ALTER TABLE study_reports
  ALTER COLUMN project_id SET NOT NULL,
  DROP CONSTRAINT study_reports_pkey,
  ADD CONSTRAINT study_reports_pkey PRIMARY KEY (project_id, study_id, report_id),
  ADD CONSTRAINT study_reports_relationship_check CHECK
    (relationship IN ('report_of_study', 'protocol', 'primary_outcome', 'safety_analysis', 'economic_analysis', 'follow_up')),
  ADD CONSTRAINT study_reports_study_project_fk FOREIGN KEY (project_id, study_id)
    REFERENCES studies(project_id, id) ON DELETE CASCADE,
  ADD CONSTRAINT study_reports_report_project_fk FOREIGN KEY (project_id, report_id)
    REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE;

CREATE UNIQUE INDEX study_reports_project_report_uq
  ON study_reports(project_id, report_id);
CREATE INDEX study_reports_project_study_idx
  ON study_reports(project_id, study_id, created_at, report_id);

CREATE TABLE study_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  study_id uuid NOT NULL,
  report_id uuid,
  event_type text NOT NULL CHECK (event_type IN (
    'study_created', 'study_renamed', 'report_assigned', 'report_moved',
    'report_unassigned', 'study_classified'
  )),
  before_study_id uuid,
  result_study_id uuid,
  before_revision bigint NOT NULL CHECK (before_revision >= 0),
  result_revision bigint NOT NULL CHECK (result_revision >= 0),
  before_snapshot jsonb NOT NULL DEFAULT '{}'::jsonb,
  result_snapshot jsonb NOT NULL DEFAULT '{}'::jsonb,
  payload jsonb NOT NULL DEFAULT '{}'::jsonb,
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (jsonb_typeof(before_snapshot) = 'object'),
  CHECK (jsonb_typeof(result_snapshot) = 'object'),
  CHECK (jsonb_typeof(payload) = 'object'),
  FOREIGN KEY (project_id, study_id) REFERENCES studies(project_id, id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id),
  FOREIGN KEY (project_id, before_study_id) REFERENCES studies(project_id, id),
  FOREIGN KEY (project_id, result_study_id) REFERENCES studies(project_id, id)
);

CREATE INDEX study_events_project_study_created_idx
  ON study_events(project_id, study_id, created_at, id);
CREATE INDEX study_events_project_report_created_idx
  ON study_events(project_id, report_id, created_at, id);

ALTER TABLE documents
  ADD CONSTRAINT documents_project_report_id_uq UNIQUE (project_id, report_id, id);
ALTER TABLE document_blocks
  ADD CONSTRAINT document_blocks_document_id_id_uq UNIQUE (document_id, id);

CREATE TABLE appraisal_assessments (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL,
  report_id uuid NOT NULL,
  definition_id text NOT NULL CHECK (length(btrim(definition_id)) BETWEEN 1 AND 100),
  definition_version integer NOT NULL CHECK (definition_version > 0),
  responses jsonb NOT NULL CHECK (jsonb_typeof(responses) = 'object'),
  judgments jsonb NOT NULL CHECK (jsonb_typeof(judgments) = 'object'),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
  completed_at timestamptz NOT NULL DEFAULT now(),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, report_id, id),
  FOREIGN KEY (project_id, report_id)
    REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE
);

CREATE INDEX appraisal_assessments_project_report_created_idx
  ON appraisal_assessments(project_id, report_id, created_at DESC, id);

CREATE TABLE appraisal_assessment_evidence (
  id uuid PRIMARY KEY,
  assessment_id uuid NOT NULL,
  project_id uuid NOT NULL,
  report_id uuid NOT NULL,
  question_id text NOT NULL CHECK (length(btrim(question_id)) > 0),
  document_id uuid NOT NULL,
  block_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (assessment_id, question_id, document_id, block_id),
  FOREIGN KEY (assessment_id, project_id, report_id)
    REFERENCES appraisal_assessments(id, project_id, report_id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, report_id, document_id)
    REFERENCES documents(project_id, report_id, id) ON DELETE CASCADE,
  FOREIGN KEY (document_id, block_id)
    REFERENCES document_blocks(document_id, id) ON DELETE CASCADE
);

CREATE INDEX appraisal_evidence_project_report_idx
  ON appraisal_assessment_evidence(project_id, report_id, assessment_id);

CREATE TABLE appraisal_events (
  id uuid PRIMARY KEY,
  assessment_id uuid NOT NULL,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL,
  event_type text NOT NULL CHECK (event_type = 'appraisal_completed'),
  payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (assessment_id),
  FOREIGN KEY (assessment_id, project_id, report_id)
    REFERENCES appraisal_assessments(id, project_id, report_id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, report_id)
    REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE
);

CREATE INDEX appraisal_events_project_report_created_idx
  ON appraisal_events(project_id, report_id, created_at DESC, id);
