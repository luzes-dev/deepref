-- phase: expand
-- Stages 4 and 6 of the existing-project migration (handoff 03a): the batched
-- backfill primitive, one bounded first batch, and the foreign key. Stages 3
-- and 6 stay in separate migrations on purpose (03_TENANCY_AUTHORIZATION.md,
-- "Do not combine steps 3 and 6 in one migration").
--
-- This migration deliberately does NOT finish the backfill. A migration is one
-- transaction, so looping over every row here would hold every row lock until
-- the end and discard all progress on a failure. Instead:
--   * `backfill_project_workspaces_batch` moves one bounded batch per call;
--   * the operator-run `deepref-server backfill-project-workspaces` command
--     calls it in a transaction per batch (resumable, safe to rerun) and then
--     validates counts in one statement;
--   * until that completes, `projects_workspace_default_trigger` (0087)
--     guarantees that no new insert, old binary or new, writes a NULL.
-- NOT NULL on `projects.workspace_id` is a contract change
-- (12_OPERATIONS_ACCEPTANCE.md) and waits for a later `-- phase: contract`
-- migration that runs only after the command reports a clean validation.

-- Moves up to `batch_size` projects that still have no workspace into the
-- migration workspace, in one statement. Idempotent: a row that has a
-- workspace is never touched. Returns how many rows it moved; zero means the
-- backfill is complete.
CREATE FUNCTION backfill_project_workspaces_batch(batch_size integer DEFAULT 500)
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
  moved integer;
BEGIN
  IF batch_size IS NULL OR batch_size < 1 THEN
    RAISE EXCEPTION 'batch_size must be at least 1';
  END IF;
  UPDATE projects
  SET workspace_id = 'f45d8021-6271-556e-a59a-fd2e0a739f95'
  WHERE id IN (
    SELECT id FROM projects WHERE workspace_id IS NULL ORDER BY id LIMIT batch_size
  );
  GET DIAGNOSTICS moved = ROW_COUNT;
  RETURN moved;
END;
$$;

-- One bounded batch, so a small or empty table is finished by the migration
-- itself. Nothing depends on this call completing the work.
SELECT backfill_project_workspaces_batch(500);

-- Existing users keep access to the projects they had before workspaces: each
-- becomes an owner of the shared migration workspace, as a `local` grant an
-- operator can narrow after cutover. Idempotent: a user who already has any
-- membership in the migration workspace is left alone.
INSERT INTO workspace_memberships (id, workspace_id, user_id, role, status, grant_source)
SELECT gen_random_uuid(), 'f45d8021-6271-556e-a59a-fd2e0a739f95', u.id, 'owner', 'active', 'local'
FROM users u
WHERE NOT EXISTS (
  SELECT 1 FROM workspace_memberships m
  WHERE m.workspace_id = 'f45d8021-6271-556e-a59a-fd2e0a739f95'
    AND m.user_id = u.id
);

-- Stage 6 (references only; NOT NULL is deferred, see the header). The key is
-- added NOT VALID so this statement does not scan `projects` while holding its
-- strong lock; 0090 validates it under a lock that lets writes continue. New
-- and updated rows are checked from this point on; a NULL passes a foreign key.
ALTER TABLE projects
  ADD CONSTRAINT projects_workspace_id_fkey
  FOREIGN KEY (workspace_id) REFERENCES workspaces (id) NOT VALID;
