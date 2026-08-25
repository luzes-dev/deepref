-- PR11 AI foundation. This migration is additive and safe for the legacy
-- evidence workspace: failed/running attempts remain audit rows, completed
-- reuse is an indexed lookup rather than a uniqueness constraint.
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS document_block_embeddings (
  document_block_id uuid NOT NULL,
  model_identifier text NOT NULL CHECK (length(btrim(model_identifier)) > 0),
  generation text NOT NULL CHECK (length(btrim(generation)) > 0),
  content_hash text NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
  dimension integer NOT NULL CHECK (dimension > 0),
  embedding vector NOT NULL,
  is_current boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (document_block_id, model_identifier, generation, content_hash),
  FOREIGN KEY (document_block_id) REFERENCES document_blocks(id) ON DELETE CASCADE,
  CHECK (vector_dims(embedding) = dimension),
  UNIQUE (document_block_id, model_identifier, generation, content_hash)
);

CREATE UNIQUE INDEX IF NOT EXISTS document_block_embeddings_current_uq
  ON document_block_embeddings(document_block_id) WHERE is_current;
CREATE INDEX IF NOT EXISTS document_block_embeddings_lookup_idx
  ON document_block_embeddings(document_block_id, content_hash, is_current, created_at DESC);
CREATE INDEX IF NOT EXISTS document_block_embeddings_hnsw_idx
  ON document_block_embeddings USING hnsw ((embedding::vector(1536)) vector_cosine_ops)
  WHERE is_current AND dimension = 1536;

ALTER TABLE ai_runs
  ADD COLUMN IF NOT EXISTS profile text,
  ADD COLUMN IF NOT EXISTS model_version text,
  ADD COLUMN IF NOT EXISTS parameters jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN IF NOT EXISTS schema_version text,
  ADD COLUMN IF NOT EXISTS prompt_hash text,
  ADD COLUMN IF NOT EXISTS schema_hash text,
  ADD COLUMN IF NOT EXISTS reuse_hash text,
  ADD COLUMN IF NOT EXISTS protocol_hash text,
  ADD COLUMN IF NOT EXISTS document_hash text,
  ADD COLUMN IF NOT EXISTS evidence_hash text,
  ADD COLUMN IF NOT EXISTS evidence_refs jsonb NOT NULL DEFAULT '[]'::jsonb,
  ADD COLUMN IF NOT EXISTS input_tokens bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS output_tokens bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cost_micros bigint,
  ADD COLUMN IF NOT EXISTS error_code text,
  ADD COLUMN IF NOT EXISTS error_message text,
  ADD COLUMN IF NOT EXISTS parent_automation_run_id uuid;

UPDATE ai_runs
SET profile = COALESCE(NULLIF(profile, ''), 'legacy'),
    model_version = COALESCE(NULLIF(model_version, ''), model),
    schema_version = COALESCE(NULLIF(schema_version, ''), 'legacy.v1'),
    prompt_hash = COALESCE(NULLIF(prompt_hash, ''), encode(digest(prompt_version, 'sha256'), 'hex')),
    schema_hash = COALESCE(NULLIF(schema_hash, ''), encode(digest(COALESCE(schema_version, 'legacy.v1'), 'sha256'), 'hex')),
    parameters = CASE WHEN jsonb_typeof(parameters) = 'object' THEN parameters ELSE '{}'::jsonb END,
    evidence_refs = CASE WHEN jsonb_typeof(evidence_refs) = 'array' THEN evidence_refs ELSE '[]'::jsonb END,
    reuse_hash = COALESCE(NULLIF(reuse_hash, ''), encode(digest(concat_ws(':', task_kind, provider, model, model_version, prompt_version, prompt_hash, schema_version, schema_hash, input_hash), 'sha256'), 'hex'));

ALTER TABLE ai_runs
  ALTER COLUMN profile SET NOT NULL,
  ALTER COLUMN model_version SET NOT NULL,
  ALTER COLUMN schema_version SET NOT NULL,
  ALTER COLUMN prompt_hash SET NOT NULL,
  ALTER COLUMN schema_hash SET NOT NULL,
  ALTER COLUMN reuse_hash SET NOT NULL,
  DROP CONSTRAINT IF EXISTS ai_runs_status_check,
  DROP CONSTRAINT IF EXISTS ai_runs_parameters_object_check,
  DROP CONSTRAINT IF EXISTS ai_runs_evidence_refs_array_check,
  DROP CONSTRAINT IF EXISTS ai_runs_reuse_hash_check,
  ADD CONSTRAINT ai_runs_status_check CHECK (status IN ('running', 'completed', 'failed', 'abstained')),
  ADD CONSTRAINT ai_runs_profile_check CHECK (length(btrim(profile)) > 0),
  ADD CONSTRAINT ai_runs_parameters_object_check CHECK (jsonb_typeof(parameters) = 'object'),
  ADD CONSTRAINT ai_runs_evidence_refs_array_check CHECK (jsonb_typeof(evidence_refs) = 'array'),
  ADD CONSTRAINT ai_runs_reuse_hash_check CHECK (reuse_hash ~ '^[0-9a-f]{64}$'),
  ADD CONSTRAINT ai_runs_prompt_hash_check CHECK (prompt_hash ~ '^[0-9a-f]{64}$'),
  ADD CONSTRAINT ai_runs_schema_hash_check CHECK (schema_hash ~ '^[0-9a-f]{64}$'),
  ADD CONSTRAINT ai_runs_input_tokens_check CHECK (input_tokens >= 0),
  ADD CONSTRAINT ai_runs_output_tokens_check CHECK (output_tokens >= 0);

ALTER TABLE ai_runs
  DROP CONSTRAINT IF EXISTS ai_runs_task_kind_provider_model_prompt_version_input_hash_key;
DROP INDEX IF EXISTS ai_runs_task_kind_provider_model_prompt_version_input_hash_key;
DROP INDEX IF EXISTS ai_runs_reuse_hash_uq;
CREATE INDEX IF NOT EXISTS ai_runs_reuse_completed_idx
  ON ai_runs(reuse_hash, completed_at DESC, created_at DESC, id)
  WHERE status = 'completed';
CREATE UNIQUE INDEX IF NOT EXISTS ai_runs_id_project_uq ON ai_runs(id, project_id);
CREATE INDEX IF NOT EXISTS ai_runs_project_created_idx ON ai_runs(project_id, created_at DESC, id);
CREATE INDEX IF NOT EXISTS ai_runs_task_status_idx ON ai_runs(task_kind, status, created_at DESC, id);

CREATE TABLE IF NOT EXISTS ai_model_routes (
  id uuid PRIMARY KEY,
  profile text NOT NULL CHECK (length(btrim(profile)) > 0),
  provider text NOT NULL CHECK (length(btrim(provider)) > 0),
  model text NOT NULL CHECK (length(btrim(model)) > 0),
  model_version text NOT NULL CHECK (length(btrim(model_version)) > 0),
  parameters jsonb NOT NULL DEFAULT '{}'::jsonb,
  enabled boolean NOT NULL DEFAULT true,
  effective_from timestamptz NOT NULL DEFAULT now(),
  effective_until timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (jsonb_typeof(parameters) = 'object'),
  CHECK (effective_until IS NULL OR effective_until > effective_from),
  UNIQUE (profile, provider, model, model_version, effective_from)
);
CREATE INDEX IF NOT EXISTS ai_model_routes_resolution_idx
  ON ai_model_routes(profile, enabled, effective_from DESC, id);

ALTER TABLE documents
  ADD CONSTRAINT documents_project_id_id_uq UNIQUE (project_id, id);

CREATE TABLE IF NOT EXISTS ai_run_evidence (
  ai_run_id uuid NOT NULL,
  project_id uuid NOT NULL,
  document_id uuid NOT NULL,
  document_block_id uuid NOT NULL,
  rank integer NOT NULL CHECK (rank > 0),
  retrieval_score double precision NOT NULL CHECK (retrieval_score >= 0),
  content_hash text NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (ai_run_id, document_block_id),
  UNIQUE (ai_run_id, rank),
  FOREIGN KEY (ai_run_id, project_id) REFERENCES ai_runs(id, project_id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, document_id) REFERENCES documents(project_id, id) ON DELETE RESTRICT,
  FOREIGN KEY (document_id, document_block_id) REFERENCES document_blocks(document_id, id) ON DELETE RESTRICT
);
CREATE INDEX IF NOT EXISTS ai_run_evidence_block_idx ON ai_run_evidence(document_id, document_block_id, ai_run_id);

ALTER TABLE ai_proposals
  ADD COLUMN IF NOT EXISTS entity_type text,
  ADD COLUMN IF NOT EXISTS entity_id uuid,
  ADD COLUMN IF NOT EXISTS operation text,
  ADD COLUMN IF NOT EXISTS model_run_id uuid,
  ADD COLUMN IF NOT EXISTS authority_tier text,
  ADD COLUMN IF NOT EXISTS resolved_at timestamptz,
  ADD COLUMN IF NOT EXISTS resolved_by_actor_kind text,
  ADD COLUMN IF NOT EXISTS resolved_by_actor_id text,
  ADD COLUMN IF NOT EXISTS resolution_reason text;

UPDATE ai_proposals p
SET entity_type = COALESCE(NULLIF(entity_type, ''), proposal_type),
    operation = COALESCE(NULLIF(operation, ''), proposal_type),
    model_run_id = COALESCE(model_run_id, ai_run_id),
    authority_tier = COALESCE(NULLIF(authority_tier, ''), 'workflow_suggestion'),
    resolved_at = CASE WHEN status = 'pending' THEN NULL ELSE COALESCE(resolved_at, decided_at, created_at) END,
    resolved_by_actor_kind = CASE WHEN status = 'pending' THEN NULL ELSE COALESCE(resolved_by_actor_kind, 'user') END,
    resolved_by_actor_id = CASE WHEN status = 'pending' THEN NULL ELSE COALESCE(NULLIF(resolved_by_actor_id, ''), NULLIF(decided_by, ''), 'legacy-migration') END;

ALTER TABLE ai_proposals
  ALTER COLUMN entity_type SET NOT NULL,
  ALTER COLUMN operation SET NOT NULL,
  ALTER COLUMN model_run_id SET NOT NULL,
  ALTER COLUMN authority_tier SET NOT NULL,
  DROP CONSTRAINT IF EXISTS ai_proposals_status_check,
  DROP CONSTRAINT IF EXISTS ai_proposals_payload_object_check,
  DROP CONSTRAINT IF EXISTS ai_proposals_resolution_shape_check,
  ADD CONSTRAINT ai_proposals_status_check CHECK (status IN ('pending', 'accepted', 'rejected', 'expired')),
  ADD CONSTRAINT ai_proposals_payload_object_check CHECK (jsonb_typeof(payload) = 'object'),
  ADD CONSTRAINT ai_proposals_authority_tier_check CHECK (authority_tier IN ('read_only', 'reversible_metadata', 'workflow_suggestion', 'scientific_conclusion')),
  ADD CONSTRAINT ai_proposals_resolution_actor_shape_check CHECK (
    (status = 'pending' AND resolved_at IS NULL AND resolved_by_actor_kind IS NULL AND resolved_by_actor_id IS NULL)
    OR (status <> 'pending' AND resolved_at IS NOT NULL
        AND resolved_by_actor_kind IN ('user', 'automation', 'system')
        AND resolved_by_actor_id IS NOT NULL AND length(btrim(resolved_by_actor_id)) > 0)
  ),
  ADD CONSTRAINT ai_proposals_run_alias_check CHECK (ai_run_id = model_run_id);

ALTER TABLE ai_proposals
  DROP CONSTRAINT IF EXISTS ai_proposals_model_run_project_fk,
  ADD CONSTRAINT ai_proposals_model_run_project_fk FOREIGN KEY (model_run_id, project_id) REFERENCES ai_runs(id, project_id) ON DELETE RESTRICT;
CREATE UNIQUE INDEX IF NOT EXISTS ai_proposals_model_run_uq ON ai_proposals(model_run_id);
CREATE INDEX IF NOT EXISTS ai_proposals_project_status_idx ON ai_proposals(project_id, status, created_at DESC, id);
CREATE INDEX IF NOT EXISTS ai_proposals_entity_idx ON ai_proposals(project_id, entity_type, entity_id, created_at DESC, id);
