# Security Architecture

DeepRef is a multi-tenant software-as-a-service (SaaS) application designed for systematic literature reviews with reproducible, accountable AI assistance. This document describes the security boundaries, authentication and authorization architecture, tenant isolation, and audit logging.

All production database infrastructure, object storage, and backup systems reside in the São Paulo region (see `docs/saas-program/BUSINESS_DECISIONS.md` for compliance and retention policies).

## Authentication Boundary

User identity and sessions are managed through WorkOS.

- **Configuration:** WorkOS integration is configured via `WORKOS_CLIENT_ID`, `WORKOS_API_KEY`, `WORKOS_COOKIE_PASSWORD`, and `WORKOS_WEBHOOK_SECRET` in `crates/config/src/workos.rs`, gated by `DEEPREF_AUTH_ENABLED`.
- **Fail-fast validation:** In production (`APP_ENV=production`), enabling authentication requires a complete WorkOS configuration. Sealed session encryption requires a cookie password of at least 32 characters (`MIN_COOKIE_PASSWORD_BYTES = 32`).
- **Session management:** Authenticated requests present an encrypted WorkOS session cookie. Session validation checks `users.status` on each request by primary key so that revoked or deactivated accounts cannot make subsequent calls even if their session cookie has not yet expired.
- **User deactivation:** When a user is deleted in WorkOS, an incoming webhook event triggers an atomic compare-and-set transitioning `users.status` to `deactivated`. In the same database transaction, a `user.deactivated` event is written to `admin_audit_events`. Deactivation invalidates active sessions while preserving user references on historical review actions, screening decisions, and provenance records.
- **Local development:** When `APP_ENV=development`, authentication can be disabled (`DEEPREF_AUTH_ENABLED=0`) to allow local development and testing without third-party cloud services.

## Public Webhook and Inbound E-mail Boundary

DeepRef exposes unauthenticated public endpoints to receive external automation triggers and inbound e-mails (`crates/http-api/src/routes/workflows.rs`):

- **Endpoints:**
  - `POST /hooks/workflows/{token}`: starts a workflow execution triggered by webhook payloads.
  - `POST /hooks/email/{token}`: parses inbound e-mails from providers (Postmark, Mailgun) and extracts literature identifiers (DOIs, PubMed IDs).
- **Payload size limit:** Inbound requests are limited to 256 KB (`MAX_HOOK_BODY_BYTES = 262_144`). Payloads exceeding this limit receive HTTP 413 Payload Too Large.
- **Token resolution:** The URL `{token}` is resolved against configured workflows in PostgreSQL (`find_workflow_by_webhook_token` / `find_workflow_by_email_token`). Unrecognized tokens return HTTP 404 Not Found.
- **Feature gate:** Webhook processing requires niche workflow nodes to be enabled (`DEEPREF_WORKFLOW_NICHE_NODES=true`). If disabled, calls return HTTP 403 Forbidden.
- **Rate limiting:** An in-memory sliding window throttles incoming calls to 60 requests per minute per address (`HOOK_RATE_LIMIT_PER_MINUTE = 60`). Excess requests are rejected with HTTP 429 Too Many Requests and a `Retry-After: 60` header.
- **HMAC-SHA256 signatures:** When a workflow requires signed payloads (`target.signature_required`), the caller must provide an `X-DeepRef-Signature` header matching `sha256=<hex HMAC-SHA256 of raw body, keyed with workflow secret>`. Unsigned or invalid requests return HTTP 403 Forbidden.
- **Idempotency:** Webhook deliveries accept an `Idempotency-Key` header, deduplicating repeated provider attempts to prevent duplicate workflow executions.

## API Keys and Service Principals (planned)

Under handoff 11 Part A, API keys and service/machine principals (`Principal::Service`) are trigger-gated after launch:

- **Machine credentials:** Service principals authenticate machine-to-machine integrations without sharing human user credentials.
- **Storage:** Only cryptographic hashes (`secret_hash`) are stored in the database. Full API key secrets are returned once upon creation and never persisted in plaintext.
- **Key prefixes:** Keys use distinct prefixes (such as `dr_live_` or `dr_dev_`) to assist secret scanners and human operators.
- **Authorization integration:** Service principals are evaluated by the same central authorization service as human principals, constrained by explicit workspace boundaries and granular scopes.
- **Scientific accountability:** Operations requiring accountable human judgment reject service principals unless an explicit automated workflow is configured and recorded in provenance.

## Secret Handling Expectations

Application secrets and sensitive credentials follow strict protection practices:

- **In-memory redaction:** Secrets (such as WorkOS API keys and cookie passwords, database connection URLs, SMTP credentials, and webhook signing secrets) are wrapped in `secrecy::SecretString` in `crates/config`. The `Debug` trait redacts secret values to prevent leakage in log files or diagnostic dumps.
- **Environment isolation:** Secrets are passed through environment variables or secure secret managers, never checked into Git or stored in Docker build layers.
- **Audit segregation:** Administrative audit logs, tracing events, error responses, and research exports must never record session cookies, access/refresh tokens, raw session identifiers, `Authorization` headers, or provider API secrets.

## Origin Protection and Cross-Site Request Defense

DeepRef implements layered defenses against cross-site request forgery (CSRF) and DNS rebinding:

- **Canonical public origin:** Defined by `DEEPREF_PUBLIC_ORIGIN` (`crates/config/src/origin.rs`). It normalizes scheme, host, and port, rejecting extraneous paths, query strings, or credentials. In production (`APP_ENV=production`), `https://` is mandatory.
- **Host header validation:** The `reject_unknown_hosts` middleware (`crates/http-api/src/host_guard.rs`) guards against DNS rebinding attacks. Incoming `Host` and HTTP/2 `:authority` headers are validated against an allow-list comprising loopback addresses (`localhost`, `127.0.0.1`, `[::1]`), the configured bind IP, and domains listed in `API_CORS_ORIGINS`. Unmatched requests are rejected with HTTP 403 Forbidden.
- **CORS allowlist:** `API_CORS_ORIGINS` controls cross-origin browser access, requiring explicit listing of frontends, proxies, or ingress gateways.
- **Origin guard:** Mutating state requests verify browser `Origin` and `Sec-Fetch-Site` headers to ensure requests originate from the application's authenticated origin.

## Tenant Isolation

DeepRef enforces multi-tenant boundary isolation across all data access:

- **Tenancy model:** Tenancy is organized hierarchically: `User -> WorkspaceMembership -> Workspace -> Project`. Each systematic review project belongs to exactly one workspace.
- **Central authorization:** Authorization decisions pass through a central Policy Decision Point / Policy Enforcement Point (PDP/PEP) using typed permissions and a route/resource coverage matrix.
- **Cross-workspace protection:** Every route confirms that the authenticated caller holds an active membership and sufficient privileges in the project's owning workspace. Cross-workspace access without membership is rejected.
- **Scientific provenance independence:** Provenance records (`record_provenance`, `ai_opinion_exposures`, `ai_screening_audit_labels`) track scientific contributions separately from administrative roles, preserving review history regardless of subsequent membership changes.

# Administrative and security audit (handoff 11 Part B)

## 1. The administrative audit stream

`admin_audit_events` (migration 0085, widened through 0142) is an append-only
stream, separate from scientific provenance:

- **Append-only.** UPDATE, DELETE and TRUNCATE are refused by triggers
  (migration 0085), with one narrow exception: the operator retention purge
  (section 5).
- **Closed shapes.** Every row's `event_type` and `metadata` are checked
  against closed lists (migration 0141, validated in 0142). No free text from
  a caller can reach the table.
- **Safe context on every row:** `request_id` (the request correlation ID,
  `x-correlation-id`, or the integration-inbox receipt for webhook-driven
  changes), `principal_kind`/`principal_user_id`, `workspace_id`,
  `project_id` and `target_user_id` — all present since 0085/0089, none added
  in 11. Every row is written in the same transaction as the change it
  records.

### What is never stored

```text
session cookies            stored only as a keyed hash (`session_ref`), never the cookie
access/refresh tokens      never
API key secrets            never (key lifecycle only, when Part A lands)
Authorization headers      never
webhook secrets            never
payment-card data          never (billing is provider-side and undecided)
raw provider credentials   never (provider sessions appear as `session_ref`)
```

Caller-supplied strings never reach the audit. Where an identifier is
recorded (a denied project, an invitation) it is stored only after parsing,
and rejection/denial rows carry no request text at all: a fixed vocabulary
plus a minute-bucket number.

---

## 2. Event inventory

Every administrative and security action in the code, and where its audit row
is written. "Bound" marks events whose volume is attacker-influenced and
therefore sampled (section 4).

| Action | Event | Written in | Bound |
| --- | --- | --- | --- |
| Sign-in succeeds | `auth.login` | `deepref_app::identity::sign_in` | |
| First sign-in provisions a user | `user.provisioned` | `identity::sign_in`, `identity::ensure_development_principal` | |
| Sign-out | `auth.logout`, `auth.session_revoked` (`user_logout`) | `identity::record_logout` | |
| Session rejected (malformed, unknown/expired/revoked, mismatched identity, invalid) | `auth.rejected` (stage `session`) | `deepref_http_api::auth::session` | yes |
| Session refresh ends the session | `auth.rejected` (stage `refresh`) | `auth::session::refresh_expired` / `refresh_and_persist` | yes |
| Sign-in callback refused | `auth.rejected` (stage `callback`) | `routes/auth.rs::callback_failure` | yes |
| Cross-origin cookie-authenticated request refused | `auth.rejected` (stage `origin`) | `origin_guard.rs` | yes |
| Authorization denied (route unlisted, platform-operator route, missing permission) | `authorization.denied` | `deepref_app::authorization::DenialAuditor` (03b) | yes |
| Provider reports the user deleted (04a seam) | `user.deactivated` (`provider_event`), `auth.session_revoked` (`user_deactivated`) | `identity::apply_provider_user_deleted`, `identity::deactivate_user` | |
| Account deletion requested | `account.deletion_requested` | `account_deletion::request_account_deletion` | |
| Account deletion cancelled | `account.deletion_cancelled` | `account_deletion::cancel_account_deletion` | |
| Account deletion completed (30 days later) | `account.deletion_completed`, `user.deactivated` (`account_deletion`), `auth.session_revoked` (`user_deactivated`) | `account_deletion::finalize_account` | |
| Workspace created | `workspace.created` | `workspaces::create_personal_workspace`, `apply_organization_snapshot` | |
| Workspace renamed | `workspace.renamed` | `workspaces::rename_workspace` | |
| Workspace deleted | `workspace.deleted`, `membership.removed`, `invitation.revoked` | `workspaces::delete_workspace` | |
| Membership added / role changed / activated / removed | `membership.created` / `.role_changed` / `.status_changed` / `.removed` | `workspaces::{add_workspace_member, change_membership_role, set_membership_status, remove_membership}`, invitation acceptance, WorkOS reconciliation | |
| Project access granted / role changed / revoked | `project_access.granted` / `.role_changed` / `.removed` | `workspaces::{grant_project_access, change_project_access_role, revoke_project_access}` | |
| Invitation created / accepted / revoked / expired | `invitation.created` / `.accepted` / `.revoked` / `.expired` | `workspaces::invite_to_workspace`, `accept_in_transaction`, `revoke_invitation` | |
| Project created | `project.created` | `routes/projects.rs::create_project` (via `record_project_created`) | |
| Project deleted (and its content) | `project.deleted` | `routes/projects.rs::delete_project`, `account_deletion::complete_workspace` (via `record_project_deleted`) | |
| Legacy actor mapped / denied (operator CLI) | `legacy_actor.mapped` / `.non_identity_recorded` | `legacy_actors::{map_legacy_actor, record_legacy_actor_non_identity}` (03c) | |
| Retention purge run | `audit.purged` | `audit_admin::purge_admin_audit` | |

### Gaps closed in 11 Part B

- **Authentication rejections** (attacker-controlled): failed callback,
  unusable session, terminal refresh failure and origin-guard rejection were
  logged but not audited. They now write `auth.rejected`, bounded like 03b's
  denials.
- **Account deletion**: `account.deletion_requested`, `.deletion_cancelled`
  and `.deletion_completed` did not exist.
- **Project lifecycle**: creating and deleting a project (content-destroying)
  left no administrative trace. `project.created` / `project.deleted` now do.
- **Denial request IDs**: handler-side and dispatch-side denials
  (`require_for_handler`, `authorize_audited`) recorded no correlation ID;
  `record_denial` now fills it from the request task-local.
- **The purge itself**: deleting audit rows is an administrative action;
  each real run writes `audit.purged` with the count.

Already covered before 11 and re-checked here: workspace rename and delete,
all membership/invitation/project-access events, provisioning, login/logout,
session revocation, provider-driven deactivation and the sampled
`authorization.denied`.

Not audited by design: read-only queries (including `audit query` itself),
retryable provider outages (logged, no session verdict), and the one-time
`backfill-project-workspaces` operator step from 03a. Settings writes have no
successful path: `PATCH /settings` is reserved for a platform operator no user
holds, and every attempt is audited as `authorization.denied`
(`platform_operator`).

---

## 3. Safe context, never secrets

`request_id` is the correlation ID the `correlation` middleware stores on
every request (echoed as `x-correlation-id`); webhook-driven changes store the
integration-inbox receipt instead. Operator commands (`deepref-server audit
purge`, `accounts complete-deletions`, `legacy-actor ...`) have no request and
store none.

`session_ref` is `SessionRefKey::session_ref(provider_session_id)` — a keyed
hash (32 bytes) of the provider session ID, never the cookie, token or sealed
credential. A test (`identity_postgres::audit_rows_hold_a_keyed_hash_and_no_secrets`)
and the forbidden-secret scan (`auth_rejection_audit::a_flood_of_authentication_rejections_stores_a_bounded_number_of_rows`
and the flows it exercises) assert that no cookie, login state, provider code
or Authorization value reaches any audit row.

---

## 4. Volume bounding under attack

Both caller-influenced event families are sampled the same way (03b's
design): one row per key per one-minute window, claimed process-locally and
enforced across processes by a unique index, inserted with
`INSERT ... ON CONFLICT DO NOTHING`. A flood stores a handful of rows, never a
row per request.

| Event | Key | Index |
| --- | --- | --- |
| `authorization.denied` | (user, permission, resource type, window) | `admin_audit_denial_window_idx` (0100) |
| `auth.rejected` | (stage, reason, target user, window) | `admin_audit_auth_rejection_window_idx` (0113) |

An `auth.rejected` target is a user id read from a stored session row, never
from the request, so an attacker's random cookies share one anonymous key
(`target IS NULL`) and cannot create new keys. A failed audit insert is
logged and retried on the next rejection: a rejection is never turned into an
error by its own audit.

---

## 5. Retention and the purge

BUSINESS_DECISIONS fixes retention:

| Rows | Retention |
| --- | --- |
| Administrative and security audit (`admin_audit_events`) | 6 months |
| `account.deletion_completed` (proof a deletion request was fulfilled) | 5 years, no content |
| `fulfilled_account_deletions` (user id, completion time, normalized-email HMAC-SHA256 digests) | 5 years; pseudonymous keyed data, no plaintext email |

Archival is not needed: the rows are the record, and the purge removes them
when their retention ends.

**Mechanism.** The append-only trigger (rewritten in migration 0112) honours
a transaction-local setting, `SET LOCAL deepref.audit_purge = 'on'`, and only
for rows already past their retention cutoff — 6 months, or 5 years for
`account.deletion_completed`. Everything else is still refused: every
UPDATE, every TRUNCATE, every DELETE outside the purge, and every purge
attempt at a row inside its window. Because the setting is `SET LOCAL` it
cannot leak past its transaction, and because the cutoff is re-checked row by
row in the trigger, no caller with the setting can remove retained history.

This was chosen over a SECURITY DEFINER purge function owned by the
migration role because the purge must run from the same operator command and
connection pool as everything else (there is no separate privileged role in
the deployment), while the narrow gate gives the same guarantee that matters
here: *retained rows are unremovable*. The trade-off is explicit: a caller
who can open a transaction can delete rows that are already past retention —
which is exactly what the purge is meant to do — and nothing else.

**Command and cadence.** Run daily (a cron entry or the platform scheduler),
with a dry run first when the query is new:

```bash
deepref-server audit purge --dry-run   # report only
deepref-server audit purge             # delete in batches of 1000
```

Each batch is its own transaction; the command is safe to interrupt and rerun
and prints a JSON report (`batches`, `audit_rows_purged`,
`fulfilled_deletion_rows_purged`, `session_rows_purged`). Every real run records `audit.purged`.

---

## 6. Incident response: querying the audit

```bash
deepref-server audit query [--workspace <uuid>] [--principal <uuid>] [--target <uuid>]
                           [--event-type <type>] [--since <rfc3339>] [--until <rfc3339>]
                           [--limit <n>]
```

Prints one JSON object per line, newest first. Filters combine with AND and
every one of them is indexed: workspace and event type by
`admin_audit_events_workspace_time_idx` / `admin_audit_events_event_type_time_idx`
(migrations 0114–0115), principal and target by the indexes of 0085, time
ranges by `admin_audit_events_occurred_idx`. The indexes are built
`CONCURRENTLY` in `-- no-transaction` migrations; an interrupted build is
refused by the guard in migration 0116 (recovery: `docs/architecture.md`,
"Migration recovery").

There is deliberately no HTTP route for this query yet: no principal holds
`PlatformOperator` (03b), so an operator HTTP surface needs its own design.
Follow-up, together with "Download my data".

---

## 7. Audit never leaks into research exports

The eleven research export kinds (`crates/http-api/src/routes/exports*`) read
project-scoped scientific tables only; `audit.csv` is the *scientific* audit
export (`deepref_app::audit_export`, built from `screening_events`,
`study_events`, `appraisal_events`, `ai_runs`, …) and has no administrative
rows. Proven by `exports_admin_audit_exclusion::no_research_export_carries_administrative_audit_data`
(one sentinel row of every administrative event type, all export kinds
requested, nothing appears) and
`no_export_module_reads_the_administrative_audit` (no export module mentions
the table).

---

## 8. "Delete my account"

Per BUSINESS_DECISIONS and handoff 11:

1. **Request** — `POST /me/deletion-request` (Authenticated: the caller
   schedules their own deletion). Inserts an `account_deletion_requests` row in
   state `requested` and records one `account.deletion_requested`. Idempotent.
2. **Cancel** — `DELETE /me/deletion-request`, only from `requested`. Moves the
   request to `cancelled` and records `account.deletion_cancelled`. From
   `completing` or `completed` it fails with `409 DELETION_IRREVERSIBLE`. A
   fresh request after a cancellation is a new row, so the 30 days restart.
3. **Complete** — `deepref-server accounts complete-deletions`, daily. Per
   account, in separate transactions:
   - **claim**: lock the request row, then the user row, re-check
     `state = 'requested' AND requested_at <= now() - 30 days` and move the
     request to `completing` before any destructive work;
   - **per workspace**: take `lock_workspace` (the lock 03a's membership
     writers and workspace deletion use) and re-check under it that the user
     is an active `owner`, that no other active membership exists, and that
     it is not the migration workspace. Then, in ONE transaction, delete the
     projects and their content, revoke pending invitations (audited) and
     delete the workspace. If any check fails, only the user's membership is
     removed; a shared workspace whose last owner leaves is left ownerless for
     the operator recovery flow (as with 03a's global deactivation) and is
     listed in `workspaces_left_ownerless` in the command output. A sole
     non-owner membership never deletes a workspace;
   - **workspace lock order**: deletion locks the workspace row, then the
     affected user rows in UUID order with `FOR KEY SHARE`, then invitations,
     then memberships. It takes those user locks before touching invitations
     because membership audit inserts reference those users. Reconciliation
     locks the workspace row before memberships. Workspace cleanup runs in its
     own transaction, outside the request/user lock transactions;
   - **finalize** (one transaction, request lock then user lock): anonymize
     the user (email and display name `Former member`, `verified_email`
     cleared, WorkOS link tombstoned as `deleted:<user uuid>`, `deleted_at`
     set), delete every session row (they hold the sealed provider payload),
     delete pending invitations addressed to the user's normalized-address
     digests and rewrite the address of the other matching invitation rows to
     `deleted-<invitation id>@invalid`, redact native legacy-decision sources
     and the PII in audit metadata (below), log
     `fulfilled_account_deletions`, move the request to `completed`, and
     write `user.deactivated` (`source: account_deletion`),
     `auth.session_revoked` (`user_deactivated`) and the deletion-proof
     `account.deletion_completed`;
   - **keeps** the stable `UserId`, every scientific provenance reference
     (contributions in shared workspaces stay attributed to the anonymized
     "former member") and the whole audit history.

#### State machine and lock order

```text
requested --cancel--> cancelled                (terminal)
requested --claim-->  completing --> completed (terminal; never due again)
```

Cancellation and completion take the same locks in the same order: **request
row, then user row** (workspace completion takes the workspace lock on its own,
never while holding either). Completion is due only from `requested` past 30
days, or resuming `completing`; `completed` is never due, whatever happens to
the five-year proof row.

The global lock order is: deletion request row before its user row; workspace
row before affected user rows in UUID order, before invitation rows, before
membership rows. Workspace deletion takes `FOR KEY SHARE` on its actor and
membership users before deleting invitations, so it cannot hold an invitation
while waiting for finalization's user lock. Reconciliation takes the workspace
row before its `FOR UPDATE` membership scan. Finalization takes request, user,
then session/invitation rows; it never holds a workspace lock.

#### PII erasure

- `users`: see finalize above. The durable marker is `users.deleted_at`, set at
  completion and never purged. A BEFORE UPDATE trigger
  (`users_anonymized_account_guard`, migration 0111) re-asserts the anonymized
  form whenever `OLD.deleted_at IS NOT NULL`, so N-1 raw sign-in SQL and
  restores cannot write PII back. A later WorkOS sign-in by the same person
  creates a NEW user.
- Audit metadata that can hold a name is the `legacy_actor_id` and `source`
  keys of `legacy_actor.mapped` / `legacy_actor.non_identity_recorded` rows
  about the user. Completion replaces them with `redacted` through
  `redact_deleted_user_audit(user_id)`, a SECURITY DEFINER function that
  refuses unless the user is already marked deleted or the deletion request is
  `completing`. It sets the transaction-local gate internally. The append-only
  trigger (migration 0117) accepts only that user's rows, only those two keys
  and only that value, with every other column unchanged. Public execution is
  revoked; a separate `deepref_app` role receives only this function when that
  role exists. A database superuser can still set the custom GUC directly;
  the trigger also checks the target user and deletion state, and only a
  superuser can bypass those database controls.
- Native `legacy_actor_mappings` and `legacy_actor_non_identities` keep their
  actor ids, user ids, operator ids and confirmation timestamps. Completion
  redacts their free-text `source` fields to `redacted`.
- The five-year fulfillment log stores lowercase hex HMAC-SHA256 digests of
  normalized primary and verified email addresses, never plaintext. These
  values are pseudonymous and linkable under the same key; the log is not
  contentless or anonymized. `DEEPREF_DELETION_DIGEST_KEY` is a `SecretString`
  used only by `deepref-server accounts complete-deletions` for first
  completion and restore replay. Production must provide it; a fixed default
  is available only in `APP_ENV=development`. Keep it in the operator secret
  store and inject it only into the completion/replay command. The key is
  never sent to PostgreSQL. Replay computes invitation HMACs in process and
  matches only invitation rows with `created_at` before the stored
  `fulfilled_account_deletions.completed_at`, so a later registration using
  the same address keeps its invitations.
- Retired keys make existing digests unmatchable. If key rotation is needed,
  retain a protected list of old keys so operators can replay earlier records;
  multi-key rotation is not implemented.
- Actual privilege separation for `redact_deleted_user_audit` also depends on
  deployment roles: migrations must run as a function/table owner distinct
  from the runtime role. The runtime role may receive only the constrained
  function grant; running the application as the migration owner removes this
  boundary.
- Sessions are also purged 30 days after expiry or revocation
  (`deepref-server audit purge`, `session_rows_purged`).

The completion command is idempotent and safe to interrupt: every step before
the finalize is idempotent and the fulfillment is recorded in the same
transaction as the anonymization. Audit rows are written only when a deletion
is fulfilled for the first time.

### Restore step

Backups roll over 35 days; a restore can bring deleted personal data back.
`fulfilled_account_deletions` is the standing log that prevents it (keep a
copy in the ops records, including the address digests and no plaintext):

1. Restore the backup as usual.
2. Make sure the restore carries the `fulfilled_account_deletions` log
   (re-import the ops copy if the restored point predates it).
3. Run `deepref-server accounts complete-deletions`.

Every logged deletion is replayed on every run, regardless of the profile
fields: sessions and invitation addresses are cleaned again from the stored
email digests, solely-owned
workspaces recreated since (restore or WorkOS reconciliation) are deleted
again, and the users row is anonymized again (and held there by the trigger
if `deleted_at` survived). Restored `requested`/`completing` requests of a
logged deletion end `completed`. No audit row is written twice.

### Out of scope here

- **"Download my data"** (ZIP of JSON plus uploaded files) — follow-up.
- Billing and AI usage records are kept 5 years linked to the billing account
  rather than the person; billing is undecided (BUSINESS_DECISIONS) and no
  `billing.*` events exist yet.
- Inactive free accounts (warning at 22 months, deletion at 24) — follow-up.

---

## 9. Boundaries documented elsewhere

- **Authentication boundary and CSRF/origin protection:**
  `crates/http-api/src/auth/mod.rs`, `crates/http-api/src/origin_guard.rs`.
- **Tenant isolation and authorization (the policy table and PEP):**
  `docs/authorization-matrix.md`, `crates/http-api/src/authz/`.
- **Public webhook signature boundary:** `crates/http-api/src/routes/workflows`,
  migrations 0071–0072.
- **API-key handling:** handoff 11 Part A (gated; not implemented).

### Audit migration ordering

The unreleased 11B audit CHECK expansion and validation are 0141 and 0142
(previously 0109 and 0110). Released migrations 0121 and 0134 rebuild these
checks, so the combined contract must follow them. Migration 0141 preserves
0134’s complete AI metadata rules, including both shapes of
`ai.hard_caps_disabled`, and adds the security/account events. Migrations
0111–0117 install tables, triggers, functions and indexes without emitting
audit events; they do not require the expanded checks to run first. Both a
fresh install and a database already at main through 0140 use this ordering.
