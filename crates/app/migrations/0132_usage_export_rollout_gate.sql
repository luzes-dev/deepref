-- phase: expand
-- Usage export rollout gate (handoff 09 slice B). A job kind an N-1 binary
-- cannot decode is dead-lettered by its claim, so a new kind may only be
-- scheduled once every deployed worker understands it. The durable activation
-- flag below is that gate, and each worker heartbeat declares the job kinds
-- its binary decodes so an operator command can check the fleet before
-- flipping it. A heartbeat without the declaration belongs to a binary that
-- predates capability reporting and is treated as understanding no new kind.
ALTER TABLE ai_usage_rollout_state
  ADD COLUMN IF NOT EXISTS ai_usage_export_activated boolean NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS ai_usage_export_activated_at timestamptz,
  ADD COLUMN IF NOT EXISTS ai_usage_export_deactivated_at timestamptz;

COMMENT ON COLUMN ai_usage_rollout_state.ai_usage_export_activated IS
  'Durable gate for scheduling ai_usage_export_replay jobs. Set only by `deepref-server ai-usage enable-export`, cleared by `disable-export` before any rollback.';

ALTER TABLE worker_heartbeats
  ADD COLUMN IF NOT EXISTS known_job_kinds text[];

COMMENT ON COLUMN worker_heartbeats.known_job_kinds IS
  'Job kinds this worker binary decodes, written with every heartbeat. NULL means the binary predates capability declaration.';
