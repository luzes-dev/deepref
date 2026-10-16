-- Applying the project's AI autonomy to a fresh proposal (record the second
-- reviewer's opinion, or act and notify) used to run after the review run's
-- finalization commit as a best effort. A crash in between lost that opinion
-- for good (review ai F13). The obligation is now written in the finalization
-- transaction and removed once applied; the reconciler drains what is left.
CREATE TABLE ai_autonomy_outbox (
  project_id uuid NOT NULL,
  proposal_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  last_error text,
  PRIMARY KEY (project_id, proposal_id),
  FOREIGN KEY (project_id, proposal_id)
    REFERENCES ai_proposals(project_id, id) ON DELETE CASCADE
);
CREATE INDEX ai_autonomy_outbox_created_idx ON ai_autonomy_outbox(created_at);
