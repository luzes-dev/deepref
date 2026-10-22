-- phase: expand
-- Security/account audit coverage (handoff 11 Part B). The event list and the
-- per-event metadata check are rebuilt under their existing names and added
-- NOT VALID, so the rebuild does not scan the audit table under an exclusive
-- lock; 0142 validates them. An N-1 binary never writes the new events, so
-- both releases run against this schema.
--
-- These unmerged 11B checks originally occupied 0109/0110. They move after
-- released 0121/0134 because those migrations rebuild the same named checks.
-- This file preserves every 0134 AI event and metadata shape (including N-1
-- hard-caps disable metadata) and adds the 11B clauses, on both fresh installs
-- and upgrades already at 0140. No released migration is modified.
-- 0111-0117 create only tables, triggers, functions and indexes; none emits
-- these audit events or depends on these expanded checks during migration.
--
-- New coverage:
--   auth.rejected             authentication rejections (failed callback,
--                             unusable session, terminal refresh failure,
--                             origin-guard rejection), attacker-controlled and
--                             therefore sampled like authorization.denied
--   account.deletion_*        "Delete my account": request, cancellation and
--                             the 30-day completion
--   project.created/deleted   project lifecycle, including the content delete
--                             an account completion performs
--
-- `user.deactivated` grows a second source: an account completion deactivates
-- the user too, and the row must say which seam did it.
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events DROP CONSTRAINT admin_audit_events_check;

ALTER TABLE admin_audit_events ADD CONSTRAINT admin_audit_events_event_type_check CHECK (event_type IN (
  'auth.login',
  'auth.logout',
  'auth.rejected',
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
  'project_access.removed',
  'project.created',
  'project.deleted',
  'invitation.created',
  'invitation.accepted',
  'invitation.revoked',
  'invitation.expired',
  'authorization.denied',
  'legacy_actor.mapped',
  'legacy_actor.non_identity_recorded',
  'account.deletion_requested',
  'account.deletion_cancelled',
  'account.deletion_completed',
  'audit.purged',
  'ai.usage_limit_changed', 'ai.usage_overage_waived', 'ai.hard_caps_enabled',
  'ai.hard_caps_disabled', 'ai.usage_reservation_resolved',
  'ai.usage_export_enabled', 'ai.usage_export_disabled'
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
      AND metadata ? 'source'
      AND metadata - 'source' = '{}'::jsonb
      AND metadata->>'source' IN ('provider_event', 'account_deletion')
      AND workspace_id IS NULL AND project_id IS NULL)
  -- Authentication rejections (11 Part B). Attacker-controlled, so the row is
  -- a fixed shape exactly like authorization.denied: `stage` and `reason` come
  -- from closed per-stage lists and `window` is the minute bucket (epoch
  -- minutes). No cookie, token, provider code, state value, origin header or
  -- other caller-supplied text is ever stored; the session appears only as
  -- `target_user_id` when a stored session row already named the user.
  -- 0113 makes one row per (stage, reason, target user, window) the most the
  -- table can hold.
  OR (event_type = 'auth.rejected'
      AND principal_kind = 'system' AND principal_user_id IS NULL
      AND workspace_id IS NULL AND project_id IS NULL
      AND metadata ? 'stage' AND metadata ? 'reason' AND metadata ? 'window'
      AND metadata - 'stage' - 'reason' - 'window' = '{}'::jsonb
      AND metadata->>'window' ~ '^[0-9]{1,10}$'
      AND ((metadata->>'stage' = 'callback'
            AND metadata->>'reason' IN (
              'invalid_state', 'access_denied', 'invalid_code', 'provider_unavailable',
              'provisioning_failed', 'account_deactivated'))
        OR (metadata->>'stage' = 'session'
            AND metadata->>'reason' IN (
              'malformed_session_cookie', 'unknown_expired_or_revoked_session',
              'session_user_mismatch', 'session_reference_mismatch', 'invalid_session'))
        OR (metadata->>'stage' = 'refresh'
            AND metadata->>'reason' IN (
              'refresh_rejected', 'session_revoked_during_refresh',
              'session_user_mismatch', 'session_reference_mismatch'))
        OR (metadata->>'stage' = 'origin'
            AND metadata->>'reason' IN (
              'multiple_origin_headers', 'origin_mismatch', 'sec_fetch_site_mismatch',
              'missing_origin_evidence'))))
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
  -- Project lifecycle (11 Part B): created and deleted, including the content
  -- deletion an account completion performs. The row names the workspace and
  -- the project only; the actor is the principal.
  OR (event_type IN ('project.created', 'project.deleted')
      AND metadata = '{}'::jsonb
      AND workspace_id IS NOT NULL AND project_id IS NOT NULL AND target_user_id IS NULL)
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
  -- "Delete my account" (11, BUSINESS_DECISIONS). The request and its
  -- cancellation are the user's own actions and name themselves as target.
  -- The completion is run 30 days later by an operator command: system
  -- principal, target is the deleted account. The completion row is the
  -- deletion-proof compliance record (kept five years, see 0112's purge gate).
  OR (event_type IN ('account.deletion_requested', 'account.deletion_cancelled')
      AND metadata = '{}'::jsonb
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL
      AND target_user_id = principal_user_id
      AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'account.deletion_completed'
      AND metadata = '{}'::jsonb
      AND principal_kind = 'system' AND principal_user_id IS NULL
      AND target_user_id IS NOT NULL
      AND workspace_id IS NULL AND project_id IS NULL)
  -- The retention purge is itself an administrative action (11 Part B): each
  -- non-dry run of `deepref-server audit purge` records that it ran and how
  -- many rows it removed. The count is a JSON number rendered as digits.
  OR (event_type = 'audit.purged'
      AND metadata ? 'rows_purged'
      AND metadata - 'rows_purged' = '{}'::jsonb
      AND jsonb_typeof(metadata->'rows_purged') = 'number'
      AND metadata->>'rows_purged' ~ '^[0-9]{1,10}$'
      AND workspace_id IS NULL AND project_id IS NULL AND target_user_id IS NULL)
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
  OR (event_type = 'ai.hard_caps_enabled'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND metadata ? 'quiet_window_seconds'
      AND metadata - 'quiet_window_seconds' = '{}'::jsonb
      AND jsonb_typeof(metadata->'quiet_window_seconds') = 'number'
      AND (metadata->>'quiet_window_seconds')::numeric > 0
      AND workspace_id IS NULL AND project_id IS NULL)
  -- Both disable commands record how many queued jobs they parked. The empty
  -- metadata shape stays valid: an N-1 binary's disable writes it.
  OR (event_type = 'ai.hard_caps_disabled'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND (metadata = '{}'::jsonb
        OR (metadata ? 'parked_jobs'
            AND metadata - 'parked_jobs' = '{}'::jsonb
            AND jsonb_typeof(metadata->'parked_jobs') = 'number'
            AND (metadata->>'parked_jobs')::numeric >= 0))
      AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'ai.usage_export_enabled'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND metadata ? 'quiet_window_seconds'
      AND metadata - 'quiet_window_seconds' = '{}'::jsonb
      AND jsonb_typeof(metadata->'quiet_window_seconds') = 'number'
      AND (metadata->>'quiet_window_seconds')::numeric > 0
      AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'ai.usage_export_disabled'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND metadata ? 'parked_jobs'
      AND metadata - 'parked_jobs' = '{}'::jsonb
      AND jsonb_typeof(metadata->'parked_jobs') = 'number'
      AND (metadata->>'parked_jobs')::numeric >= 0
      AND workspace_id IS NULL AND project_id IS NULL)
  OR (event_type = 'ai.usage_reservation_resolved'
      AND principal_kind = 'user' AND principal_user_id IS NOT NULL AND target_user_id IS NULL
      AND metadata ? 'reservation_id' AND metadata ? 'resolution'
      AND metadata->>'reservation_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      AND metadata->>'resolution' IN ('settle', 'void')
      AND ((metadata->>'resolution' = 'settle'
            AND metadata ? 'cost_micros'
            AND metadata - 'reservation_id' - 'resolution' - 'cost_micros' = '{}'::jsonb
            AND jsonb_typeof(metadata->'cost_micros') = 'number'
            AND (metadata->>'cost_micros')::numeric >= 0)
        OR (metadata->>'resolution' = 'void'
            AND metadata ? 'reason'
            AND metadata - 'reservation_id' - 'resolution' - 'reason' = '{}'::jsonb
            AND jsonb_typeof(metadata->'reason') = 'string'
            AND length(btrim(metadata->>'reason')) BETWEEN 1 AND 500))
      AND workspace_id IS NOT NULL)
) NOT VALID;
