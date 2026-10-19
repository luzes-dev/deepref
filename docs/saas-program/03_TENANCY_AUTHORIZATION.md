# Handoff 03: workspace tenancy, authorization and scientific identity

## Goal

Introduce the real multi-tenant authorization model.

A WorkOS-authenticated person becomes a DeepRef user. A user gains access through workspace/project membership. Scientific reviewer assignment remains a separate domain concern.

Read `00_PROGRAM.md`. Complete handoffs 01 and 02 first.

## Deliverables and release gate

This handoff has three deliverables:

```text
03a  workspace tables, memberships, onboarding, staged project backfill
03b  central authorization: PDP/PEP, typed permissions, coverage matrix
03c  principal -> actor, scientific assignments and blinding, identity continuity
```

They ship as one release gate. Develop and merge them in order, but do not enable multi-user access to a production workspace until all three are complete.

Reason: 03b authorizes who may reach a project. If scientific writes still accept an arbitrary reviewer identity (`x-actor-id`, read in `crates/http-api/src/routes/actor.rs` and sent by `apps/web/src/lib/api/assistant-stream.ts` and `apps/web/src/lib/api/custom-fetch.ts`), any authorized member can act as any other reviewer. Blinding, consensus and fresh-audit eligibility then rest on a label the caller chose. 03a and 03b alone produce tenants with a hole in the scientific record.

## Scope

Implement:

- `WorkspaceId` and workspace persistence;
- WorkOS Organization mapping;
- workspace memberships, invitations and onboarding;
- migration of existing projects into a workspace;
- project-to-workspace ownership;
- central authorization service;
- typed permissions;
- project access rules;
- route/resource coverage matrix with a foreign-workspace test per row;
- authenticated `Principal -> Actor`;
- removal of `x-actor-id`/arbitrary reviewer identity from live writes;
- minimum scientific assignment model needed to support real reviewers and blinding;
- legacy-actor to `UserId` identity continuity;
- audit events for membership, role and permission changes and for authorization denials.

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

## 03a: workspaces, onboarding and project backfill

### Authority split

WorkOS is authoritative for external authentication and organization provisioning.

DeepRef PostgreSQL is authoritative for runtime application access decisions.

Normal project requests must not call WorkOS.

Synchronize WorkOS Organization/member state through local projections.

### Existing-project migration

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

Each stage must be compatible with the previous application release (expand/contract, see `12_OPERATIONS_ACCEPTANCE.md`). Do not combine steps 3 and 6 in one migration.

### Workspace roles

Keep the set small initially:

```text
owner
admin
member
```

These are account/product roles.

Do not encode every capability as a role.

### Project access

Add project membership only if current product requirements need access narrower than workspace membership.

Possible roles:

```text
project_admin
member
viewer
```

Choose the smallest set supported by current flows.

### Onboarding and first workspace

What creates the first workspace for a new user is a product decision. Default until a product decision replaces it:

```text
first sign-in without a pending invitation -> create a personal workspace, user is owner
first sign-in with a pending invitation    -> join the inviting workspace, create no personal workspace
```

Create the user, the workspace and the owner membership in one transaction, keyed on the WorkOS user ID so a retried callback creates nothing twice. Map the workspace to a WorkOS Organization when one exists; do not make a normal request depend on creating one.

Invitations:

```text
workspace:members:invite creates a pending invitation (workspace, email, role, expiry)
acceptance binds the invitation to the authenticated user and creates the membership
invitations expire, can be revoked, and are single use
the inviter cannot grant a role above their own
```

### WorkOS reconciliation

Apply WorkOS Organization and membership events through the integration inbox defined in `04_NATS_OUTBOX_FOUNDATION.md` (04a): authenticated receipt, deduplication, ordered processing. Webhooks alone are not sufficient. Add periodic reconciliation that lists Organizations and memberships from WorkOS and repairs local projections that drifted (missed, reordered or failed events).

Reconciliation rules:

- the local projection only moves forward; an older event never overwrites newer state;
- a membership missing from WorkOS is deactivated locally, not deleted;
- a WorkOS Organization with no local workspace is mapped or created through the same path as onboarding;
- a conflict between WorkOS state and a local DeepRef-only grant is surfaced as a reconciliation finding, not silently resolved.

### Membership removal and deactivation

Removing a membership revokes access to that workspace only. The user keeps other workspaces and their account.

Revocation takes effect on the next request. No cached authorization (in-process cache, session claim, Valkey entry) may outlive a removal or role downgrade. Either read membership from PostgreSQL on each authorization decision, or key any cache on a membership version that the removal bumps in the same transaction.

Global user deactivation (account-level, distinct from workspace removal):

```text
rejects every new request for the user in every workspace
ends the user's sessions (coordinated with 02)
preserves the user row and all historical provenance
does not rewrite scientific events, assignments or audit rows
```

Deactivating or removing the last `owner` of a workspace is refused until ownership is transferred.

### Workspace-scoped jobs

Workspace-scoped background work (reconciliation, workspace-level exports, billing projections) cannot use the queue today: `EnqueueJob` requires a `ProjectId` (`crates/application/src/jobs.rs`) and claiming excludes null projects (`crates/app/src/jobs.rs`). Workspace-scoped jobs depend on the typed project/workspace/platform job scope introduced in 05a. Until then, run such work as an explicit non-queue task or as a project-scoped job; do not add a placeholder project ID.

## 03b: central authorization

### Architecture

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

Fail closed. An unknown role, unknown status, missing membership, or resource that cannot be resolved to a workspace is a denial.

### Cross-tenant security

Every resource load used by an authenticated command must establish tenant scope.

Do not trust a URL `project_id` merely because it exists.

Use resource-aware authorization before mutation or sensitive read. Resolve the resource to its workspace (through the project) inside the same query or transaction as the load, so a guessed ID of a foreign workspace is indistinguishable from a missing one.

### Route and resource coverage matrix

Maintain a checked-in matrix that lists every route and every non-HTTP access path with its required `Permission`, its `Resource` type and the test that proves a foreign-workspace principal is denied. One row per entry; no row is shared.

Cover:

```text
reads                      list and get endpoints, including search and pagination cursors
writes                     create, update, delete, bulk and import endpoints
SSE streams                assistant run events and any other stream; authorize on connect and on resume
downloads and exports      article files, workflow files, generated exports
object access              object-store keys and any signed URL issued by the API
machine principals         worker/system principals, and API keys when 11 Part A lands
background-job payloads    handlers that load a project or workspace from job data
```

Every row has a foreign-workspace test: a principal in workspace B calls the entry with identifiers of workspace A and receives the same response as for a nonexistent resource.

Add a check that fails when a route is registered without a matrix entry. Generate the route list from the router (or the OpenAPI document, `pnpm generate:api`) and compare it to the matrix; the check runs in CI and in `cargo xtask boundaries` or an equivalent existing gate. A deliberate exemption (health, auth callbacks) is an explicit matrix entry with a reason, never an absent one.

### Evaluate PostgreSQL RLS as a separate decision

Do not adopt row-level security as part of this handoff, and do not rule it out. Record the decision separately after 03b lands, with these constraints stated:

- pooled connections: per-request tenant context must be set and reset safely on pooled SQLx connections (transaction-local settings only), or a leaked setting crosses tenants;
- privileged workers: workers and migrations act across workspaces and would need a bypass role, which becomes the new trust boundary;
- tenant scope is derived via `project`: most scientific tables carry `project_id`, not `workspace_id`, so a policy needs a join or denormalized column and a performance review.

The application-level PDP plus the coverage matrix is the control for this handoff. RLS, if adopted, is defense in depth on top of it.

### Scientific policy

Keep contextual rules in domain/application policy objects.

Examples:

```text
Can this reviewer see another reviewer's decision?
Has the reviewer submitted their blinded decision?
Can they enter consensus?
Has prior AI exposure disqualified a fresh audit?
```

These are not generic RBAC permissions.

### Audit

Audit begins here, not in 11. Emit these into the administrative audit table defined in `02_WORKOS_IDENTITY.md`, in the same transaction as the change:

```text
membership created, role changed, status changed, removed
invitation created, accepted, revoked, expired
workspace created, renamed, deleted
global user deactivated or reactivated
authorization denials
```

Denials are high volume and attacker-controlled. Bound them: sample or aggregate per (principal, permission, resource type, window), cap stored metadata, never store request bodies or raw identifiers supplied by the caller beyond fixed-length fields. Membership, role and permission changes are never sampled.

Keep administrative audit separate from scientific provenance (`screening_events` and similar). 11 owns the remaining audit coverage, query API and retention.

## 03c: principal to actor, assignments and identity continuity

### Scientific assignments

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

### Principal to Actor

New scientific writes derive the actor from the authenticated principal.

Humans use canonical `UserId`.

Internal automation/system/AI actors remain server-created.

External callers cannot claim those actor kinds. HTTP already rejects client-supplied `system`/`automation` kinds (`client_actor_kind` in `crates/http-api/src/routes/actor.rs`); what remains is the self-asserted human `x-actor-id`, defaulting to `local-user`. Remove `x-actor-id` and `x-actor-kind` as authority: ignore them, then delete the header from `assistant-stream.ts`, `custom-fetch.ts` and `currentReviewerId()` once the server no longer reads it.

Preserve old self-asserted history without rewriting it.

### Identity continuity

Historical actor IDs are free text. `crates/app/migrations/0055_ai_first_blind_controls.sql` compares them directly: `ai_first_fresh_auditor(cohort, actor text)` checks `approved_by`, `screening_events.actor_id` (where `actor_kind='user'`) and `ai_opinion_exposures.audience_actor_id`; `ai_first_queue_visible(project, report, actor text)` also compares `ai_screening_audit_labels.actor_id`. Callers are in `crates/app/src/screening.rs` and `crates/app/src/ai_first/audit.rs`. If a new `UserId` is passed as `actor`, none of those comparisons match the same person's old label, and a reviewer who was exposed under `reviewer-a` appears fresh as `UserId`. That would break fresh-audit and blinding guarantees silently.

Requirements:

- add a trusted legacy-actor to `UserId` mapping table (legacy actor kind, legacy actor ID, `UserId`, confirmed by, confirmed at, source). Rows are created only by an operator confirming a specific legacy label belongs to a specific user. Never infer a mapping from a matching name, email or the self-asserted label itself: the label was never authenticated;
- blinding and fresh-audit checks consult the mapping: a `UserId` is exposed if its own ID or any legacy actor mapped to it appears in the exposure, decision, approval or label history;
- when no trusted mapping exists for a user in an affected cohort, treat the new user as exposed and ineligible for fresh audit in that cohort. Conservative means a person may be wrongly excluded; it never means a previously exposed person is treated as fresh. Operators lift the exclusion by confirming a mapping or by recording that the user was not the labelled actor;
- the unmapped legacy labels stay valid for history and cannot be used by any new request; no session can authenticate as a legacy label;
- never rewrite historical rows. `screening_events`, `ai_opinion_exposures`, audit labels and cohort approvals keep their original text actor IDs. Canonical `UserId` is written only on new rows, and the comparison functions are changed (new migration, not an edit of 0055) to match either form through the mapping;
- the mapping is itself audited as an administrative change (see 03b Audit).

Enumerate every place that compares an actor ID textually before cutover (`grep` for `actor_id`, `audience_actor_id`, `approved_by` across `crates/app`), and give each one a test.

## Out of scope

Do not implement:

- Polar billing;
- API keys;
- NATS migration;
- broad review workflow redesign;
- FGA/OpenFGA/Cedar;
- PostgreSQL RLS (decision only, see 03b).

## Tests

Cover:

03a:

- every project has one workspace after migration;
- migration is idempotent and preserves project IDs and scientific rows;
- first sign-in creates one personal workspace exactly once, including on callback retry;
- sign-in with a pending invitation joins that workspace and creates no personal workspace;
- invitation expiry, revocation, single use and role ceiling;
- WorkOS membership events apply through the inbox in order; a stale event does not overwrite newer state;
- periodic reconciliation repairs a missed removal and a missed addition;
- removing a membership denies the next request to that workspace, with no cached decision surviving, and leaves the user's other workspaces intact;
- global deactivation rejects requests in every workspace and preserves historical provenance;
- last-owner removal is refused.

03b:

- cross-workspace ID guessing is denied for every matrix row, with the same response as a nonexistent resource;
- the matrix check fails when a route is added without an entry;
- role permission mapping;
- unknown role, unknown status and missing membership fail closed;
- project authorization;
- SSE stream: a revoked member's open stream is closed or refused on resume;
- membership, role and permission changes produce an audit row in the same transaction, and a rolled-back change produces none;
- denial auditing is bounded under a flood of denied requests.

03c:

- browser cannot forge another human actor;
- `x-actor-id` and `x-actor-kind` are ignored for authority;
- browser cannot create system/automation actor;
- scientific blinding policy with two authenticated users;
- a user whose legacy label was exposed, with a confirmed mapping, is ineligible for fresh audit;
- a user with no trusted mapping in an affected cohort is treated as exposed and ineligible;
- a mapping cannot be created from a self-asserted label without operator confirmation;
- historical rows are byte-identical after the migration and after the cutover;
- user deletion/deactivation preserves historical provenance.

## Completion criterion

DeepRef has real tenants and real users, released as one gate (03a, 03b and 03c). Every live scientific human action is attributable to an authenticated `UserId`, every project belongs to one workspace, and cross-workspace access is denied centrally and proven by a route/resource coverage matrix with a foreign-workspace test per row, rather than by frontend convention. Previously exposed reviewers are never treated as fresh because their identity changed form. Membership removal takes effect on the next request, and membership, role and permission changes and authorization denials are recorded in the administrative audit.
