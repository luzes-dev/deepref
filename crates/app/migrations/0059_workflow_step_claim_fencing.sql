-- Workflow steps are fenced on the job claim that executes them.
--
-- A step can be executed by more than one job over its life (its first job
-- and every later "look" job while it waits), and each job counts its own
-- attempts, so the claim is the pair (claimed_job_id, claimed_attempt).
-- begin_node_execution compare-and-sets the pair; finish and retry only take
-- effect while the step is still running under the same pair. A duplicate
-- delivery or a stale attempt after its lease was reclaimed can then neither
-- re-run the block's side effects nor overwrite the recorded outcome.
ALTER TABLE workflow_node_runs
  ADD COLUMN claimed_job_id uuid,
  ADD COLUMN claimed_attempt integer CHECK (claimed_attempt IS NULL OR claimed_attempt >= 1),
  ADD CONSTRAINT workflow_node_runs_claim_pair
    CHECK ((claimed_job_id IS NULL) = (claimed_attempt IS NULL));
