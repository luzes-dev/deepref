-- phase: expand
-- AI quota changes and overage waivers join the append-only administrative audit.
-- The checks remain NOT VALID until 0122, preserving the established expand/
-- validate split while N-1 continues to write only existing event kinds.
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_check;
ALTER TABLE admin_audit_events ADD CONSTRAINT admin_audit_events_event_type_check CHECK (event_type IN (
  'auth.login', 'auth.logout', 'auth.session_revoked', 'user.provisioned', 'user.deactivated',
  'workspace.created', 'workspace.renamed', 'workspace.deleted', 'membership.created',
  'membership.role_changed', 'membership.status_changed', 'membership.removed',
  'project_access.granted', 'project_access.role_changed', 'project_access.removed',
  'invitation.created', 'invitation.accepted', 'invitation.revoked', 'invitation.expired',
  'authorization.denied', 'legacy_actor.mapped', 'legacy_actor.non_identity_recorded',
  'ai.usage_limit_changed', 'ai.usage_overage_waived'
)) NOT VALID;
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
  -- Authorization denials (03b). High volume and caller-influenced, so the
  -- row is a fixed shape: the permission and resource type come from closed
  -- lists, `window` is the minute bucket (epoch minutes), and the resource
  -- identifier is stored only when it parsed as a UUID. No request body, path
  -- text or caller-supplied string is ever stored. 0100 makes one row per
  -- (user, permission, resource type, window) the most the table can hold.
  OR (event_type = 'authorization.denied'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL
      AND target_user_id IS NULL
      AND metadata ? 'permission' AND metadata ? 'resource_type' AND metadata ? 'window'
      AND metadata - 'permission' - 'resource_type' - 'window' = '{}'::jsonb
      AND metadata->>'permission' IN (
        'workspace:view', 'workspace:manage', 'workspace:billing', 'workspace:members:invite',
        'project:create', 'project:view', 'project:edit', 'project:delete',
        'screening:perform', 'screening:resolve', 'extraction:perform', 'appraisal:perform',
        'automation:manage', 'ai:use', 'api_key:manage',
        'route_unlisted', 'platform_operator')
      AND metadata->>'resource_type' IN ('project', 'workspace', 'none')
      AND metadata->>'window' ~ '^[0-9]{1,10}$'
      AND ((metadata->>'resource_type' = 'project' AND workspace_id IS NULL)
        OR (metadata->>'resource_type' = 'workspace' AND project_id IS NULL)
        OR (metadata->>'resource_type' = 'none' AND workspace_id IS NULL AND project_id IS NULL)))
  -- Legacy identity decisions (03c). The old label is retained in the audited metadata;
  -- only trusted operator commands can create these rows.
  OR (event_type IN ('legacy_actor.mapped', 'legacy_actor.non_identity_recorded')
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL
      AND target_user_id IS NOT NULL
      AND workspace_id IS NULL AND project_id IS NULL
      AND metadata ? 'legacy_actor_kind' AND metadata ? 'legacy_actor_id' AND metadata ? 'source'
      AND (NOT (metadata ? 'affected_finalized_cohorts')
        OR jsonb_typeof(metadata->'affected_finalized_cohorts') = 'array')
      AND metadata - 'legacy_actor_kind' - 'legacy_actor_id' - 'source'
        - 'affected_finalized_cohorts' = '{}'::jsonb
      AND metadata->>'legacy_actor_kind' = 'user'
      AND btrim(metadata->>'legacy_actor_id') <> ''
      AND length(metadata->>'legacy_actor_id') <= 200
      AND btrim(metadata->>'source') <> ''
      AND length(metadata->>'source') <= 200)
  OR (event_type = 'ai.usage_limit_changed'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND metadata ? 'scope_kind' AND metadata ? 'old_limit_micros' AND metadata ? 'new_limit_micros'
      AND metadata - 'scope_kind' - 'old_limit_micros' - 'new_limit_micros' = '{}'::jsonb
      AND metadata->>'scope_kind' IN ('platform', 'workspace', 'project')
      AND jsonb_typeof(metadata->'old_limit_micros') = 'number'
      AND jsonb_typeof(metadata->'new_limit_micros') = 'number'
      AND (metadata->>'old_limit_micros')::numeric >= 0
      AND (metadata->>'new_limit_micros')::numeric >= 0
      AND ((metadata->>'scope_kind' = 'platform' AND workspace_id IS NULL AND project_id IS NULL)
        OR (metadata->>'scope_kind' = 'workspace' AND workspace_id IS NOT NULL AND project_id IS NULL)
        OR (metadata->>'scope_kind' = 'project' AND workspace_id IS NOT NULL AND project_id IS NOT NULL)))
  OR (event_type = 'ai.usage_overage_waived'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND metadata ? 'overage_micros' AND metadata - 'overage_micros' = '{}'::jsonb
      AND jsonb_typeof(metadata->'overage_micros') = 'number'
      AND (metadata->>'overage_micros')::numeric > 0
      AND workspace_id IS NOT NULL AND project_id IS NULL)
) NOT VALID;
