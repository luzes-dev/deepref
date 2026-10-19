-- phase: expand
-- Workspace tenancy (handoff 03a, stages 1-3 of "Existing-project migration").
--
-- A workspace is the tenant: every project belongs to exactly one workspace,
-- and users reach projects through workspace memberships. WorkOS is
-- authoritative for external authentication and organization provisioning;
-- this database is authoritative for runtime access decisions. Membership
-- rows therefore record where each grant came from: `workos_mirrored` grants
-- follow the provider, `local` grants are DeepRef-only and are never touched
-- by reconciliation.
--
-- Staged migration, compatible with the previous application release:
--   1. workspace tables (this file);
--   2. one deterministic migration workspace for existing data (this file);
--   3. nullable `projects.workspace_id` (this file), with a trigger that
--      assigns the migration workspace to rows inserted by binaries that do
--      not know the column yet;
--   4. batched idempotent backfill (0088);
--   5. validate counts and references (0088);
--   6. foreign-key constraint (0088). NOT NULL is a contract change and is
--      deliberately deferred to a later contract migration (see 0088).

-- The tenant. `workos_organization_id` maps a workspace to one WorkOS
-- Organization; it is nullable because a personal workspace may have no
-- organization and no normal request may depend on creating one.
CREATE TABLE workspaces (
  id uuid PRIMARY KEY,
  kind text NOT NULL CHECK (kind IN ('personal', 'organization', 'migration')),
  name text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  -- Stable URL identity: renaming a workspace never moves a URL.
  slug text NOT NULL UNIQUE
    CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND length(slug) <= 80),
  workos_organization_id text UNIQUE
    CHECK (workos_organization_id IS NULL OR length(workos_organization_id) BETWEEN 1 AND 128),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

-- A user's place in one workspace. `grant_source` is the provenance of the
-- grant: absence at the provider deactivates only `workos_mirrored` rows.
-- Removal deletes the row and leaves its history in `admin_audit_events`;
-- deactivation keeps the row and only withdraws access.
CREATE TABLE workspace_memberships (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL REFERENCES workspaces (id),
  user_id uuid NOT NULL REFERENCES users (id),
  workos_membership_id text UNIQUE
    CHECK (workos_membership_id IS NULL OR length(workos_membership_id) BETWEEN 1 AND 128),
  role text NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
  status text NOT NULL CHECK (status IN ('active', 'deactivated')),
  grant_source text NOT NULL CHECK (grant_source IN ('workos_mirrored', 'local')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, user_id),
  -- Only a provider-mirrored grant carries the provider's membership ID.
  CHECK ((grant_source = 'workos_mirrored') = (workos_membership_id IS NOT NULL))
);

-- Membership is resolved per request with no cache in front, so a removal or
-- role downgrade takes effect on the next request. This index serves that
-- lookup; the partial predicate matches the only rows authorization reads.
CREATE INDEX workspace_memberships_user_idx
  ON workspace_memberships (user_id)
  WHERE status = 'active';

-- Access narrower than workspace membership, for the few flows that need it.
-- Workspace membership remains the default grant; rows here only narrow or
-- extend a user's access to one project.
CREATE TABLE project_access (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users (id),
  role text NOT NULL CHECK (role IN ('project_admin', 'member', 'viewer')),
  grant_source text NOT NULL CHECK (grant_source IN ('workos_mirrored', 'local')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, user_id)
);

CREATE INDEX project_access_user_idx ON project_access (user_id);

-- Stage 2: one deterministic migration workspace for every project that
-- existed before workspaces. The ID is fixed (UUID v5 of the documented
-- constant below) so the row is created exactly once across every rerun of
-- this stage and can be named in code and runbooks. It is matched in
-- `deepref_app::workspaces::MIGRATION_WORKSPACE_ID`.
INSERT INTO workspaces (id, kind, name, slug)
VALUES (
  'f45d8021-6271-556e-a59a-fd2e0a739f95',
  'migration',
  'Existing projects',
  'existing-projects'
)
ON CONFLICT (id) DO NOTHING;

-- Stage 3: nullable first. Rows written before this migration have no
-- workspace yet; the backfill in 0088 assigns them.
ALTER TABLE projects ADD COLUMN workspace_id uuid;

CREATE INDEX projects_workspace_idx ON projects (workspace_id);

-- Rolling safety for stage 3: a binary from the previous release inserts
-- projects without a workspace_id. Such a row goes to the migration
-- workspace, so the staged backfill and the NOT NULL of a later release never
-- disagree with live traffic. This trigger is part of the expand phase and is
-- removed only in the contract migration that retires the old insert shape.
CREATE FUNCTION assign_project_workspace_default()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  NEW.workspace_id := 'f45d8021-6271-556e-a59a-fd2e0a739f95';
  RETURN NEW;
END;
$$;

CREATE TRIGGER projects_workspace_default_trigger
BEFORE INSERT ON projects
FOR EACH ROW WHEN (NEW.workspace_id IS NULL)
EXECUTE FUNCTION assign_project_workspace_default();
