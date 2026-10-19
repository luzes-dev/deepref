-- DeepRef users and the append-only administrative audit.
--
-- `users.id` is the DeepRef identity that every later table references. The
-- identity provider's user ID (`workos_user_id`) is an attribute of the row and
-- never a key, and an email never links two provider identities. Deactivation
-- sets `status`; rows are not deleted, so `users.id` stays valid for provenance,
-- assignments, usage and audit rows.
CREATE TABLE users (
  id uuid PRIMARY KEY,
  workos_user_id text NOT NULL UNIQUE CHECK (length(workos_user_id) BETWEEN 1 AND 128),
  primary_email text NOT NULL CHECK (length(primary_email) BETWEEN 1 AND 320),
  display_name text CHECK (display_name IS NULL OR length(display_name) <= 200),
  status text NOT NULL CHECK (status IN ('active', 'deactivated')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  deleted_at timestamptz
);

-- The browser receives only a random opaque token. Its keyed digest is the
-- lookup key; the WorkOS sealed session stays server-side and is encrypted by
-- WorkOS's session format. Revocation takes effect locally without waiting
-- for the provider.
CREATE TABLE user_sessions (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users (id),
  session_token_hash bytea NOT NULL UNIQUE CHECK (length(session_token_hash) = 32),
  sealed_provider_session text NOT NULL
    CHECK (length(sealed_provider_session) BETWEEN 1 AND 16384),
  session_ref bytea NOT NULL CHECK (length(session_ref) = 32),
  -- Compare-and-set token for refresh persistence: the provider rotates the
  -- refresh token on every use, so a stalled replica must never overwrite a
  -- newer sealed session with the one it read earlier.
  version bigint NOT NULL DEFAULT 0,
  -- Short lease taken before a refresh token is spent at the provider, so one
  -- request at a time, across replicas, refreshes a session. A request that
  -- cannot take it waits for the holder's commit instead of treating a spent
  -- token as a dead session. The lease expires on its own if its holder dies.
  refresh_lease_owner uuid,
  refresh_lease_until timestamptz,
  CHECK ((refresh_lease_owner IS NULL) = (refresh_lease_until IS NULL)),
  created_at timestamptz NOT NULL DEFAULT now(),
  expires_at timestamptz NOT NULL,
  revoked_at timestamptz,
  CHECK (expires_at > created_at)
);

CREATE INDEX user_sessions_user_active_idx
  ON user_sessions (user_id, expires_at DESC)
  WHERE revoked_at IS NULL;

-- Administrative audit, separate from scientific provenance. A row is written
-- in the same transaction as the change it records. `session_ref` is a keyed
-- hash of the provider session ID, never the ID or a cookie. `metadata` takes
-- keys from a closed list per event type and holds no email, name, free text or
-- provider payload.
--
-- Foreign keys carry no cascade: users are never deleted, and the trigger below
-- refuses DELETE as well as UPDATE, so audit rows keep their `UserId`.
CREATE TABLE admin_audit_events (
  id uuid PRIMARY KEY,
  occurred_at timestamptz NOT NULL DEFAULT now(),
  event_type text NOT NULL CHECK (event_type IN (
    'auth.login',
    'auth.logout',
    'auth.session_revoked',
    'user.provisioned',
    'user.deactivated'
  )),
  principal_kind text NOT NULL CHECK (principal_kind IN ('user', 'system')),
  principal_user_id uuid REFERENCES users (id),
  target_user_id uuid REFERENCES users (id),
  session_ref bytea CHECK (session_ref IS NULL OR length(session_ref) = 32),
  request_id text CHECK (request_id IS NULL OR length(request_id) <= 200),
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
  CHECK (
    (event_type IN ('auth.login', 'auth.logout', 'user.provisioned') AND metadata = '{}'::jsonb)
    OR (event_type = 'auth.session_revoked'
        AND metadata ? 'reason'
        AND metadata - 'reason' = '{}'::jsonb
        AND metadata->>'reason' IN ('user_logout', 'user_deactivated'))
    OR (event_type = 'user.deactivated'
        AND metadata = '{"source":"provider_event"}'::jsonb)
  ),
  CHECK ((principal_kind = 'user') = (principal_user_id IS NOT NULL))
);

CREATE INDEX admin_audit_events_occurred_idx ON admin_audit_events (occurred_at DESC, id);
CREATE INDEX admin_audit_events_principal_idx
  ON admin_audit_events (principal_user_id, occurred_at DESC)
  WHERE principal_user_id IS NOT NULL;
CREATE INDEX admin_audit_events_target_idx
  ON admin_audit_events (target_user_id, occurred_at DESC)
  WHERE target_user_id IS NOT NULL;

-- Unlike ai_opinion_exposures (0049), which allows DELETE so project deletes
-- can cascade, nothing cascades into this table, so UPDATE, DELETE and
-- TRUNCATE are all refused.
CREATE FUNCTION reject_admin_audit_event_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'administrative audit events are append-only'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER admin_audit_events_append_only_trigger
BEFORE UPDATE OR DELETE ON admin_audit_events
FOR EACH ROW EXECUTE FUNCTION reject_admin_audit_event_changes();

CREATE TRIGGER admin_audit_events_no_truncate_trigger
BEFORE TRUNCATE ON admin_audit_events
FOR EACH STATEMENT EXECUTE FUNCTION reject_admin_audit_event_changes();
