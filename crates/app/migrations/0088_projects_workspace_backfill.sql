-- phase: expand
-- Stages 4-6 of the existing-project migration (handoff 03a): backfill every
-- project into the deterministic migration workspace, validate counts and
-- references, and only then add the foreign key. Stages 3 and 6 are kept in
-- separate migrations on purpose (03_TENANCY_AUTHORIZATION.md, "Do not
-- combine steps 3 and 6 in one migration").
--
-- NOT NULL on `projects.workspace_id` is a contract change per
-- 12_OPERATIONS_ACCEPTANCE.md (it needs `-- phase: contract` and a named
-- release out of service), so it is deliberately NOT applied here: this
-- release must roll out alongside the previous one. Until a later contract
-- migration adds it, `projects_workspace_default_trigger` (0087) guarantees
-- that no insert, old binary or new, writes a NULL.

-- Stage 4: batched and idempotent. Only rows still without a workspace are
-- moved, so a rerun changes nothing, and a row an old binary inserted after
-- 0087 already carries the migration workspace through the trigger. Returns
-- how many rows it moved. It stays callable after this migration as an
-- explicit non-queue maintenance step for any row written before 0087 that a
-- crashed deployment left behind.
CREATE FUNCTION backfill_project_workspaces(batch_size integer DEFAULT 500)
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
  moved integer := 0;
  batch integer;
BEGIN
  IF batch_size IS NULL OR batch_size < 1 THEN
    RAISE EXCEPTION 'batch_size must be at least 1';
  END IF;
  LOOP
    UPDATE projects
    SET workspace_id = 'f45d8021-6271-556e-a59a-fd2e0a739f95'
    WHERE id IN (
      SELECT id FROM projects WHERE workspace_id IS NULL ORDER BY id LIMIT batch_size
    );
    GET DIAGNOSTICS batch = ROW_COUNT;
    moved := moved + batch;
    EXIT WHEN batch = 0;
  END LOOP;
  RETURN moved;
END;
$$;

SELECT backfill_project_workspaces();

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

-- Stage 5: validate counts and references before the constraint. The
-- migration aborts (and this file changes nothing) if any project is left
-- unassigned or points at a workspace that does not exist.
DO $$
DECLARE
  total_projects bigint;
  total_assigned bigint;
  unassigned bigint;
  orphaned bigint;
BEGIN
  SELECT count(*) INTO total_projects FROM projects;
  SELECT count(*) INTO total_assigned
  FROM projects p JOIN workspaces w ON w.id = p.workspace_id;
  SELECT count(*) INTO unassigned FROM projects WHERE workspace_id IS NULL;
  SELECT count(*) INTO orphaned
  FROM projects p LEFT JOIN workspaces w ON w.id = p.workspace_id
  WHERE p.workspace_id IS NOT NULL AND w.id IS NULL;
  IF unassigned <> 0 OR orphaned <> 0 OR total_projects <> total_assigned THEN
    RAISE EXCEPTION
      'project workspace validation failed: total %, assigned %, unassigned %, orphaned %',
      total_projects, total_assigned, unassigned, orphaned;
  END IF;
END;
$$;

-- Stage 6 (references only; NOT NULL is deferred, see the header). The key is
-- added NOT VALID so this statement does not scan `projects` while holding its
-- strong lock; 0090 validates it under a lock that lets writes continue. New
-- and updated rows are checked from this point on.
ALTER TABLE projects
  ADD CONSTRAINT projects_workspace_id_fkey
  FOREIGN KEY (workspace_id) REFERENCES workspaces (id) NOT VALID;
