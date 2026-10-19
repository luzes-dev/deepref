-- phase: expand
-- Administrative audit coverage for workspace tenancy (handoff 03a).
--
-- 0085's table keeps its meaning and its append-only guarantee; this
-- migration only widens the closed event list and adds the two identifiers
-- tenancy events need. No existing row is touched: the identity events of
-- 0085 keep their exact metadata shapes and stay workspace-less.
--
-- `workspace_id` and `project_id` carry no foreign key on purpose. Audit rows
-- are append-only (0085 refuses UPDATE and DELETE), so they must keep an
-- identifier whose subject is gone after a workspace or project is deleted;
-- an ON DELETE action would need the UPDATE the trigger refuses.

ALTER TABLE admin_audit_events ADD COLUMN workspace_id uuid;
ALTER TABLE admin_audit_events ADD COLUMN project_id uuid;

COMMENT ON COLUMN admin_audit_events.workspace_id IS
  'The workspace a tenancy event changed. Identifier only, never a foreign key: append-only rows outlive workspaces.';
COMMENT ON COLUMN admin_audit_events.project_id IS
  'The project a project-access event changed. Identifier only, never a foreign key: append-only rows outlive projects.';

-- Both checks are added NOT VALID so the rebuild does not scan the audit table
-- under an exclusive lock. Every existing row already satisfied the narrower
-- 0085 checks and the identity shapes are unchanged here; 0090 validates them
-- under a lock that lets writes continue. Both keep the names they have had
-- since 0085, so later migrations extend them the same way instead of editing
-- 0085.
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_check;

ALTER TABLE admin_audit_events ADD CONSTRAINT admin_audit_events_event_type_check CHECK (event_type IN (
  'auth.login',
  'auth.logout',
  'auth.session_revoked',
  'user.provisioned',
  'user.deactivated',
  'workspace.created',
  'workspace.renamed',
  'workspace.deleted',
  'membership.created',
  'membership.role_changed',
  'membership.status_changed',
  'membership.removed',
  'project_access.granted',
  'project_access.role_changed',
  'project_access.removed'
)) NOT VALID;

-- The closed metadata list per event type, as in 0085: a row carries exactly
-- the keys its event allows, and those keys take closed values only. No email,
-- display name, free text or provider payload ever reaches this table.
ALTER TABLE admin_audit_events ADD CONSTRAINT admin_audit_events_check CHECK (
  -- Identity events (0085), unchanged and never workspace-scoped.
  (event_type IN ('auth.login', 'auth.logout', 'user.provisioned')
   AND metadata = '{}'::jsonb
   AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'auth.session_revoked'
      AND metadata ? 'reason'
      AND metadata - 'reason' = '{}'::jsonb
      AND metadata->>'reason' IN ('user_logout', 'user_deactivated')
      AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'user.deactivated'
      AND metadata = '{"source":"provider_event"}'::jsonb
      AND workspace_id IS NULL AND project_id IS NULL)
  -- Workspace lifecycle. `workspace_kind` is `personal` or `organization`;
  -- the migration workspace is created by a migration, not by a request.
  OR (event_type = 'workspace.created'
      AND metadata ? 'workspace_kind'
      AND metadata - 'workspace_kind' = '{}'::jsonb
      AND metadata->>'workspace_kind' IN ('personal', 'organization')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  OR (event_type IN ('workspace.renamed', 'workspace.deleted')
      AND metadata = '{}'::jsonb
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  -- Membership changes name the member in `target_user_id`.
  OR (event_type IN ('membership.created', 'membership.removed')
      AND metadata ? 'role' AND metadata ? 'grant_source'
      AND metadata - 'role' - 'grant_source' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND metadata->>'grant_source' IN ('workos_mirrored', 'local')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  OR (event_type = 'membership.role_changed'
      AND metadata ? 'role'
      AND metadata - 'role' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  OR (event_type = 'membership.status_changed'
      AND metadata ? 'status'
      AND metadata - 'status' = '{}'::jsonb
      AND metadata->>'status' IN ('active', 'deactivated')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
  -- Project-access changes name the member and the project.
  OR (event_type IN ('project_access.granted', 'project_access.removed')
      AND metadata ? 'role' AND metadata ? 'grant_source'
      AND metadata - 'role' - 'grant_source' = '{}'::jsonb
      AND metadata->>'role' IN ('project_admin', 'member', 'viewer')
      AND metadata->>'grant_source' IN ('workos_mirrored', 'local')
      AND workspace_id IS NOT NULL AND project_id IS NOT NULL AND target_user_id IS NOT NULL)
  OR (event_type = 'project_access.role_changed'
      AND metadata ? 'role'
      AND metadata - 'role' = '{}'::jsonb
      AND metadata->>'role' IN ('project_admin', 'member', 'viewer')
      AND workspace_id IS NOT NULL AND project_id IS NOT NULL AND target_user_id IS NOT NULL)
) NOT VALID;
