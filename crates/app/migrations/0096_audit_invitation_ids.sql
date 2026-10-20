-- phase: expand
-- Invitation events name their invitation (handoff 03a). Two invitations to
-- one workspace with the same role were indistinguishable in the audit; the
-- metadata now carries the invitation UUID (never the address). The check is
-- rebuilt under its existing name and added NOT VALID so the rebuild does not
-- scan the audit table under an exclusive lock; 0097 validates it.
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_check;

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
  -- Invitations (03a). Never the invitee's email: the row names the workspace
  -- and the role only. Acceptance names the accepting user; the other events
  -- have no user to name (an invitee may not have an account yet).
  OR (event_type IN ('invitation.created', 'invitation.revoked', 'invitation.expired')
      AND metadata ? 'role' AND metadata ? 'invitation_id'
      AND metadata - 'role' - 'invitation_id' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND metadata->>'invitation_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  OR (event_type = 'invitation.accepted'
      AND metadata ? 'role' AND metadata ? 'invitation_id'
      AND metadata - 'role' - 'invitation_id' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND metadata->>'invitation_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
) NOT VALID;
