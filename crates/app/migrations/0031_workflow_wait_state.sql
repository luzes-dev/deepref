-- A workflow step that waits for the AI keeps what it is waiting for here (the
-- review runs it started and its deadline), so a restarted worker resumes the
-- wait instead of starting the reviews again.
ALTER TABLE workflow_node_runs ADD COLUMN wait_state jsonb;
