# Handoff 03: workspace tenancy, authorization and scientific identity

## Goal

Introduce the real multi-tenant authorization model.

A WorkOS-authenticated person becomes a DeepRef user. A user gains access through workspace/project membership. Scientific reviewer assignment remains a separate domain concern.

Read `00_PROGRAM.md`. Complete handoffs 01 and 02 first.

## Scope

Implement:

- `WorkspaceId` and workspace persistence;
- WorkOS Organization mapping;
- workspace memberships;
- migration of existing projects into a workspace;
- project-to-workspace ownership;
- central authorization service;
- typed permissions;
- project access rules;
- authenticated `Principal -> Actor`;
- removal of `x-actor-id`/arbitrary reviewer identity from live writes;
- minimum scientific assignment model needed to support real reviewers and blinding.

## Tenant model

Target:

```text
User
  <-> WorkspaceMembership
        |
        v
     Workspace
        |
        v
      Project
```

Every project belongs to exactly one workspace.

Conceptual tables:

```sql
workspaces (
    id uuid primary key,
    workos_organization_id text unique,
    name text,
    slug text,
    created_at timestamptz,
    updated_at timestamptz
)

workspace_memberships (
    id uuid primary key,
    workspace_id uuid not null,
    user_id uuid not null,
    workos_membership_id text,
    role text not null,
    status text not null,
    created_at timestamptz,
    updated_at timestamptz,
    unique(workspace_id, user_id)
)
```

Use repository conventions rather than copying these columns mechanically.

## Authority split

WorkOS is authoritative for external authentication and organization provisioning.

DeepRef PostgreSQL is authoritative for runtime application access decisions.

Normal project requests must not call WorkOS.

Synchronize WorkOS Organization/member state through local projections.

## Existing-project migration

Use a staged migration:

1. create workspace tables;
2. create one deterministic migration workspace for existing data;
3. add nullable `project.workspace_id`;
4. backfill every project;
5. validate counts and references;
6. add `NOT NULL` and foreign-key constraints;
7. switch reads/writes;
8. remove obsolete paths only after cutover.

Preserve all project IDs and scientific records.

## Workspace roles

Keep the set small initially:

```text
owner
admin
member
```

These are account/product roles.

Do not encode every capability as a role.

## Project access

Add project membership only if current product requirements need access narrower than workspace membership.

Possible roles:

```text
project_admin
member
viewer
```

Choose the smallest set supported by current flows.

## Authorization architecture

Use Policy Enforcement Points and one central Policy Decision Point.

Conceptually:

```rust
authorization
    .require(
        &principal,
        Permission::ProjectDelete,
        Resource::Project(project_id),
    )
    .await?;
```

Create a closed typed `Permission` model.

Candidate permissions:

```text
workspace:view
workspace:manage
workspace:billing
workspace:members:invite

project:create
project:view
project:edit
project:delete

screening:perform
screening:resolve
extraction:perform
appraisal:perform
automation:manage
ai:use
api_key:manage
```

Do not spread raw permission strings or `role == "admin"` checks through handlers.

Fail closed.

## Scientific assignments

Do not make WorkOS roles represent methodology.

Model assignments separately where needed:

```text
primary screener
secondary screener
consensus reviewer
extractor
appraisal reviewer
fresh AI auditor
```

Create only the assignment structure required to replace arbitrary reviewer identities and keep blinding correct. Do not redesign the entire screening UX here.

## Scientific policy

Keep contextual rules in domain/application policy objects.

Examples:

```text
Can this reviewer see another reviewer's decision?
Has the reviewer submitted their blinded decision?
Can they enter consensus?
Has prior AI exposure disqualified a fresh audit?
```

These are not generic RBAC permissions.

## Principal to Actor

New scientific writes derive the actor from the authenticated principal.

Humans use canonical `UserId`.

Internal automation/system/AI actors remain server-created.

External callers cannot claim those actor kinds.

Preserve old self-asserted history without rewriting it.

## Cross-tenant security

Every resource load used by an authenticated command must establish tenant scope.

Do not trust a URL `project_id` merely because it exists.

Use resource-aware authorization before mutation or sensitive read.

## Out of scope

Do not implement:

- Polar billing;
- API keys;
- NATS migration;
- broad review workflow redesign;
- FGA/OpenFGA/Cedar.

## Tests

Cover:

- every project has one workspace after migration;
- cross-workspace ID guessing is denied;
- role permission mapping;
- unknown membership fails closed;
- project authorization;
- browser cannot forge another human actor;
- browser cannot create system/automation actor;
- scientific blinding policy with two authenticated users;
- user deletion/deactivation preserves historical provenance.

## Completion criterion

DeepRef has real tenants and real users. Every live scientific human action is attributable to an authenticated `UserId`, every project belongs to one workspace, and cross-workspace access is denied centrally rather than by frontend convention.
