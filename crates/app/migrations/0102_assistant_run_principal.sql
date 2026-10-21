-- phase: expand
-- Durable assistant runs carry who they run for (handoff 03b), so the worker
-- can authorize each tool call and plan action against current grants.
--
--   authz_principal = 'user'                 principal_user_id names the person
--   authz_principal = 'development_bypass'  authentication was off (local dev)
--   authz_principal IS NULL                  enqueued by a release that predates
--                                            this column: the worker fails the
--                                            run closed, never guesses a user
--
-- Both columns are nullable and unvalidated by constraint: an N-1 binary
-- inserts runs without them and keeps working; the worker treats such a run as
-- unauthorized.
ALTER TABLE assistant_agent_runs ADD COLUMN principal_user_id uuid;
ALTER TABLE assistant_agent_runs ADD COLUMN authz_principal text;
