-- Durable assistant agent runs: execution owned by the worker, observed over
-- SSE. The browser observes execution; it never owns it.
--
-- A run is created queued with its resolved model route and fixed output
-- identities (answer message id, plan id). The worker claims the durable job,
-- transitions the run to running, executes the Rig agent, persists batched
-- events, the effect log, the final answer and the plan, then marks the run
-- terminal. Lease recovery re-drives a running run from scratch; fixed output
-- identities plus conflict-safe writes keep one final answer and one plan.

CREATE TABLE assistant_agent_runs (
  id uuid PRIMARY KEY,
  conversation_id uuid NOT NULL REFERENCES assistant_conversations(id) ON DELETE CASCADE,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  trigger_message_id uuid NOT NULL REFERENCES assistant_messages(id) ON DELETE CASCADE,
  actor_kind text NOT NULL,
  actor_id text NOT NULL,
  status text NOT NULL CHECK (status IN ('queued', 'running', 'suspended', 'completed', 'failed', 'cancelled')),
  model_route jsonb NOT NULL,
  semantic_contract_id text NULL,
  runtime_version text NOT NULL,
  build_provenance jsonb NOT NULL DEFAULT '{}',
  rig_run jsonb NULL,
  effect_log jsonb NULL,
  run_spec_hash text NULL,
  answer_message_id uuid NULL,
  plan_id uuid NULL,
  error jsonb NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz NULL
);

CREATE INDEX assistant_agent_runs_conversation_idx
  ON assistant_agent_runs (conversation_id, created_at DESC);

-- Persisted turn events for reconnectable observation. Text is batched by
-- the worker (one row per flush, never one row per token).
CREATE TABLE assistant_run_events (
  run_id uuid NOT NULL REFERENCES assistant_agent_runs(id) ON DELETE CASCADE,
  seq bigint NOT NULL CHECK (seq >= 0),
  kind text NOT NULL CHECK (kind IN ('status', 'text', 'replace', 'tool_start', 'tool_complete', 'plan', 'done', 'error')),
  payload jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (run_id, seq)
);
