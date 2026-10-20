-- phase: expand
-- Separate methodology assignments from account roles and record only
-- operator-confirmed legacy identities. Existing provenance stays untouched.

CREATE TABLE scientific_assignments (
  project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users (id),
  role text NOT NULL CHECK (role IN (
    'primary_screener',
    'secondary_screener',
    'consensus_reviewer',
    'extractor',
    'appraisal_reviewer',
    'fresh_ai_auditor'
  )),
  assigned_by uuid NOT NULL REFERENCES users (id),
  created_at timestamptz NOT NULL DEFAULT now(),
  revoked_at timestamptz,
  CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE UNIQUE INDEX scientific_assignments_one_active_role
  ON scientific_assignments (project_id, user_id, role)
  WHERE revoked_at IS NULL;

CREATE INDEX scientific_assignments_active_user_role
  ON scientific_assignments (user_id, role, project_id)
  WHERE revoked_at IS NULL;

COMMENT ON TABLE scientific_assignments IS
  'Methodology roles, independent of workspace or WorkOS account roles. Revocation preserves assignment history.';

CREATE TABLE legacy_actor_mappings (
  legacy_actor_kind text NOT NULL CHECK (legacy_actor_kind = 'user'),
  legacy_actor_id text NOT NULL CHECK (
    btrim(legacy_actor_id) <> '' AND length(legacy_actor_id) <= 200
  ),
  user_id uuid NOT NULL REFERENCES users (id),
  confirmed_by uuid NOT NULL REFERENCES users (id),
  confirmed_at timestamptz NOT NULL DEFAULT now(),
  source text NOT NULL CHECK (length(btrim(source)) BETWEEN 1 AND 200),
  PRIMARY KEY (legacy_actor_kind, legacy_actor_id)
);

CREATE INDEX legacy_actor_mappings_user_idx
  ON legacy_actor_mappings (user_id, legacy_actor_kind, legacy_actor_id);

COMMENT ON TABLE legacy_actor_mappings IS
  'Trusted operator confirmations that a historical human actor label belongs to a DeepRef user.';

CREATE TABLE legacy_actor_non_identities (
  legacy_actor_kind text NOT NULL CHECK (legacy_actor_kind = 'user'),
  legacy_actor_id text NOT NULL CHECK (
    btrim(legacy_actor_id) <> '' AND length(legacy_actor_id) <= 200
  ),
  user_id uuid NOT NULL REFERENCES users (id),
  recorded_by uuid NOT NULL REFERENCES users (id),
  recorded_at timestamptz NOT NULL DEFAULT now(),
  source text NOT NULL CHECK (length(btrim(source)) BETWEEN 1 AND 200),
  PRIMARY KEY (legacy_actor_kind, legacy_actor_id, user_id)
);

CREATE INDEX legacy_actor_non_identities_user_idx
  ON legacy_actor_non_identities (user_id, legacy_actor_kind, legacy_actor_id);

COMMENT ON TABLE legacy_actor_non_identities IS
  'Trusted operator confirmations that a historical human actor label does not belong to a DeepRef user.';
