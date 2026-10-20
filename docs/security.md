# DeepRef security and account audit

The authentication boundary, secret handling, tenant isolation and the
administrative/security audit stream (handoff 11 Part B). Scientific
provenance lives elsewhere: this document covers only administrative and
security events, account deletion and their retention.

---

## 1. The administrative audit stream

`admin_audit_events` (migration 0085, widened in 0089–0116) is an append-only
stream, separate from scientific provenance:

- **Append-only.** UPDATE, DELETE and TRUNCATE are refused by triggers
  (migration 0085), with one narrow exception: the operator retention purge
  (section 5).
- **Closed shapes.** Every row's `event_type` and `metadata` are checked
  against closed lists (migration 0109, validated in 0110). No free text from
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
| `fulfilled_account_deletions` (user id, completion time, normalized-email SHA-256 digests) | 5 years; no plaintext email |

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
   - **workspace lock order**: reconciliation and deletion lock the workspace
     row before locking any membership rows. Reconciliation releases that row
     at transaction end; deletion never holds request or user locks while it
     processes workspaces;
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

Membership writers, WorkOS reconciliation, and account-deletion workspace
cleanup use **workspace row, then membership rows**. Reconciliation acquires
`lock_workspace` before its `FOR UPDATE` membership scan. This makes its lock
order match deletion, so neither transaction holds memberships while waiting
for the workspace.

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
- The fulfillment log stores lowercase hex SHA-256 digests of the normalized
  primary and verified email addresses, never plaintext. Replays hash each
  invitation's normalized address and compare the digest, even when a restore
  has replaced the users row with an unrelated profile.
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
