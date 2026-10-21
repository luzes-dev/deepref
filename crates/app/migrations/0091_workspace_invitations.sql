-- phase: expand
-- Workspace invitations (handoff 03a).
--
-- An invitation is a pending offer of a role in one workspace to an email
-- address. There is no bearer token: acceptance binds the invitation to the
-- authenticated user whose provider-verified email matches, so there is no
-- secret to store or leak. An invitation is single use (the conditional
-- UPDATE that accepts it), expires, and can be revoked. Expiry is enforced
-- when acceptance is attempted; the row is then marked `expired`.

CREATE TABLE workspace_invitations (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL REFERENCES workspaces (id),
  -- Normalized: trimmed and lowercase. Matched against the signed-in user's
  -- email with the same normalization.
  email text NOT NULL
    CHECK (email = lower(btrim(email)) AND length(email) BETWEEN 3 AND 320 AND position('@' IN email) > 1),
  role text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
  invited_by uuid NOT NULL REFERENCES users (id),
  status text NOT NULL CHECK (status IN ('pending', 'accepted', 'revoked', 'expired')),
  expires_at timestamptz NOT NULL,
  accepted_by_user_id uuid REFERENCES users (id),
  accepted_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK ((status = 'accepted') = (accepted_by_user_id IS NOT NULL AND accepted_at IS NOT NULL)),
  CHECK ((status = 'revoked') = (revoked_at IS NOT NULL))
);

-- One pending invitation per workspace and address.
CREATE UNIQUE INDEX workspace_invitations_pending_idx
  ON workspace_invitations (workspace_id, email)
  WHERE status = 'pending';

-- Sign-in looks up pending invitations by address.
CREATE INDEX workspace_invitations_email_idx
  ON workspace_invitations (email)
  WHERE status = 'pending';

-- The audit table gains the four invitation events. Same approach as 0089:
-- the checks are rebuilt under their existing names and added NOT VALID so the
-- rebuild does not scan the audit table under an exclusive lock; 0092
-- validates them. Existing rows satisfy the new checks because only
-- alternatives were added.
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
  'project_access.removed',
  'invitation.created',
  'invitation.accepted',
  'invitation.revoked',
  'invitation.expired'
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
  -- Invitations (03a). Never the invitee's email: the row names the workspace
  -- and the role only. Acceptance names the accepting user; the other events
  -- have no user to name (an invitee may not have an account yet).
  OR (event_type IN ('invitation.created', 'invitation.revoked', 'invitation.expired')
      AND metadata ? 'role'
      AND metadata - 'role' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NULL)
  OR (event_type = 'invitation.accepted'
      AND metadata ? 'role'
      AND metadata - 'role' = '{}'::jsonb
      AND metadata->>'role' IN ('owner', 'admin', 'member')
      AND workspace_id IS NOT NULL AND project_id IS NULL AND target_user_id IS NOT NULL)
) NOT VALID;
