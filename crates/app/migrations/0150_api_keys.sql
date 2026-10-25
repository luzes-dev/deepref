-- phase: expand
-- Machine credentials are distinct from browser sessions. High-entropy tokens
-- never enter the database: only their 32-byte cryptographic verifier does.
-- Cascade removes workspace credentials; the separate audit retains lifecycle
-- facts and the stable creator. Revocation and
-- expiry are checked on authentication AND each central authorization read.
CREATE TABLE api_keys (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  created_by_user_id uuid NOT NULL REFERENCES users(id),
  name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
  prefix text NOT NULL CHECK (prefix = 'dr_key_v1_'),
  secret_hash bytea NOT NULL CHECK (length(secret_hash) = 32),
  scopes text[] NOT NULL CHECK (
    cardinality(scopes) BETWEEN 1 AND 2
    AND array_position(scopes, NULL) IS NULL
    AND scopes <@ ARRAY['workspace:view', 'project:view']::text[]
  ),
  expires_at timestamptz,
  last_used_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (expires_at IS NULL OR expires_at > created_at)
);
CREATE INDEX api_keys_workspace_created_idx ON api_keys(workspace_id, created_at DESC, id);
CREATE INDEX api_keys_creator_idx ON api_keys(created_by_user_id);
COMMENT ON COLUMN api_keys.secret_hash IS
  'HMAC-SHA256 keyed by a 256-bit random secret over the fixed deepref-api-key-v1 context; constant-time verification. Never the secret.';
