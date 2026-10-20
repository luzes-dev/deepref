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
| Project deleted (and its content) | `project.deleted` | `routes/projects.rs::delete_project`, `account_deletion::delete_solely_owned_workspaces` (via `record_project_deleted`) | |
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
| `fulfilled_account_deletions` (user id + completion time) | 5 years, no content |

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
`fulfilled_deletion_rows_purged`). Every real run records `audit.purged`.

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
   schedules their own deletion). Records `users.deletion_requested_at` and
   one `account.deletion_requested`. Idempotent.
2. **Cancel** — `DELETE /me/deletion-request` any time in the next 30 days.
   Clears the request and records `account.deletion_cancelled`. Idempotent.
3. **Complete** — `deepref-server accounts complete-deletions`, daily, 30
   days after the request. Per account:
   - deletes the workspaces the user solely owns (no other member remains)
     and their content, through the existing project and workspace deletions
     (`project.deleted`, `membership.removed`, `invitation.revoked`,
     `workspace.deleted` audited); workspaces other members remain in are
     kept;
   - deactivates the account and anonymizes the PII: email and display name
     become `Former member`, `verified_email` is cleared, `deleted_at` is set;
   - **keeps** the stable `UserId`, every scientific provenance reference
     (contributions in shared workspaces stay attributed to the anonymized
     "former member") and the whole audit history;
   - revokes remaining sessions (`auth.session_revoked`), writes
     `user.deactivated` (`source: account_deletion`) and the deletion-proof
     `account.deletion_completed`, and logs the fulfillment in
     `fulfilled_account_deletions` (user id and time only).

The completion command is idempotent and safe to interrupt: workspaces are
deleted first, and the fulfillment is recorded in the same transaction as the
anonymization. A logged deletion is never recorded twice; re-application is a
restore remedy (below), not a second deletion. A fulfilled account never
re-absorbs provider PII: a later sign-in callback with the same provider
identity updates nothing (`identity::sign_in`, asserted by
`identity_postgres::a_fulfilled_deletion_never_re_absorbs_provider_pii`).

### Restore step

Backups roll over 35 days; a restore can bring deleted personal data back.
`fulfilled_account_deletions` is the standing log that prevents it (keep a
copy in the ops records — it is two columns and no content):

1. Restore the backup as usual.
2. Make sure the restore carries the `fulfilled_account_deletions` log
   (re-import the ops copy if the restored point predates it).
3. Run `deepref-server accounts complete-deletions`.

Every logged deletion is re-applied: the account is anonymized and deactivated
again, any restored solely-owned workspace and its content are deleted again,
sessions are revoked again. No audit row is written twice.

### Out of scope here

- **"Download my data"** (ZIP of JSON plus uploaded files) — follow-up.
- Billing and AI usage records are kept 5 years linked to the billing account
  rather than the person; billing is undecided (BUSINESS_DECISIONS) and no
  `billing.*` events exist yet.
- Inactive free accounts (warning at 22 months, deletion at 24) — follow-up.
- Session retention purge (`user_sessions`, 30 days after expiry/revocation)
  — follow-up; sessions are revoked but not yet purged.

---

## 9. Boundaries documented elsewhere

- **Authentication boundary and CSRF/origin protection:**
  `crates/http-api/src/auth/mod.rs`, `crates/http-api/src/origin_guard.rs`.
- **Tenant isolation and authorization (the policy table and PEP):**
  `docs/authorization-matrix.md`, `crates/http-api/src/authz/`.
- **Public webhook signature boundary:** `crates/http-api/src/routes/workflows`,
  migrations 0071–0072.
- **API-key handling:** handoff 11 Part A (gated; not implemented).
