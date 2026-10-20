# Handoff 02: WorkOS identity and authenticated sessions

## Goal

Introduce canonical DeepRef users and replace anonymous/self-asserted browser identity with WorkOS AuthKit authentication.

Read `00_PROGRAM.md` and complete handoff 01 first.

## Scope

Implement:

- canonical `UserId`;
- `users` persistence;
- WorkOS user mapping;
- login, callback and logout;
- secure browser session handling;
- origin protection for cookie-authenticated unsafe requests;
- administrative audit table, starting with the identity events;
- global user deactivation through the 04a integration inbox;
- `AuthenticatedPrincipal`;
- `/me` or equivalent current-session endpoint;
- frontend authenticated-session bootstrap;
- removal of browser-owned reviewer identity from authentication decisions.

Do not implement workspace authorization yet. That is handoff 03.

## Canonical identity

DeepRef owns the user ID.

Conceptual schema:

```sql
users (
    id uuid primary key,
    workos_user_id text not null unique,
    primary_email text not null,
    display_name text,
    status text not null,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    deleted_at timestamptz
)
```

Follow repository migration conventions.

Never use email or WorkOS user ID as a DeepRef primary key.

Introduce strongly typed IDs.

## WorkOS boundary

Use WorkOS AuthKit and the current official Rust integration where supported.

WorkOS owns:

- authentication;
- password/passkey/social/MFA flows exposed by AuthKit;
- external session lifecycle;
- enterprise SSO later.

DeepRef owns:

- canonical `UserId`;
- application session interpretation;
- authorization;
- scientific actor identity.

Provider DTOs stop in the integrations/HTTP adapter boundary.

## Browser flow

Target:

```text
browser
  -> DeepRef login endpoint
  -> WorkOS AuthKit
  -> DeepRef callback
  -> resolve/provision UserId
  -> secure HttpOnly session cookie
```

Validate current WorkOS requirements for:

```text
state
PKCE
callback URL
session integrity
return URL
```

Allow return targets only from the configured application origin or explicit safe paths.

## Session security

Do not put access or refresh tokens in:

```text
localStorage
sessionStorage
IndexedDB
```

Prefer the WorkOS-supported sealed/session-cookie mechanism when the Rust SDK supports it cleanly.

Production cookie properties must include:

```text
HttpOnly
Secure
SameSite=Lax or stricter when compatible
Path=/
```

Prefer a `__Host-` cookie name if deployment constraints permit it.

## CSRF and origin protection

`SameSite=Lax` is defense in depth, not the control. It does not stop a sibling subdomain (same site, different origin), and it still sends cookies on top-level `GET` navigations.

The control is an origin check on each unsafe request that presents the session cookie:

```text
POST, PUT, PATCH, DELETE with the session cookie:
  Origin present   -> must equal the canonical application origin from 01,
                      exactly: scheme, host and port. Origin: null is rejected.
  Origin absent    -> Sec-Fetch-Site must be same-origin.
                      same-site, cross-site and none are rejected.
  both absent      -> reject.
```

Browsers send `Origin` on unsafe requests, so a cookie-bearing request with neither header is not a browser request.

- Reject with 403 before the body is parsed or any handler runs.
- Read the canonical origin from runtime configuration, never from the `Host` header. `API_CORS_ORIGINS` (`crates/config/src/api.rs`) is not this check: it is a list, and CORS does not stop a cross-site request from being sent.
- Requests without the session cookie are not subject to this rule. They authenticate by their own mechanism (API key in 11 Part A, webhook signature in 04a), or they are rejected as unauthenticated.
- Safe methods (`GET`, `HEAD`, `OPTIONS`) change no state.
- Logout is `POST`. A `GET` logout would let any page sign the user out.
- The `state` value is bound to the browser and checked on callback, so a callback started by another site fails.
- Implement this as one middleware in `crates/http-api`, beside the host guard (`crates/http-api/src/host_guard.rs`), so every route inherits it.

## Principal model

Introduce or prepare:

```rust
pub enum Principal {
    User(UserPrincipal),
    Service(ServicePrincipal),
    System(SystemPrincipal),
}
```

This handoff implements the user path.

Keep `UserPrincipal` minimal:

```rust
pub struct UserPrincipal {
    pub user_id: UserId,
}
```

Workspace membership is not embedded in the identity token.

## Legacy actor path

Header-supplied identity is the current source of actor attribution. Producers and consumers:

```text
crates/http-api/src/routes/actor.rs               extract_actor reads x-actor-kind and
                                                  x-actor-id; defaults to local-user
apps/web/src/lib/api/assistant-stream.ts          sends both headers on assistant requests
apps/web/src/lib/api/custom-fetch.ts              sends both headers when absent
apps/web/src/lib/features/assistant/chat-api.ts   sends x-actor-kind
apps/web/src/lib/api/reviewer.ts                  stores the typed label in sessionStorage
                                                  (deepref-reviewer-id)
apps/web/src/lib/features/ai-autonomy/components/AiFirstPanel.svelte
                                                  lets the user type that label
```

`client_actor_kind` already rejects client-supplied `system` and `automation` kinds. Arbitrary human identity is still accepted: any browser can record scientific actions as any reviewer.

This handoff removes header authority:

- authentication never reads `x-actor-kind`, `x-actor-id` or `deepref-reviewer-id`;
- authorization and administrative audit use only the session's `UserId`;
- protected routes reject a request without a valid session before actor extraction, so the `local-user` default is unreachable from a browser;
- auth, session and `/me` requests carry no `x-actor-*` header.

Scientific writes still record the header label as legacy attribution until 03c. That label is not authentication, authorization or audit identity. Replacing it changes the text IDs that `crates/app/migrations/0055_ai_first_blind_controls.sql` compares, so the replacement happens only with 03c's continuity work. 03a–03c ship as one release gate, so no paying workspace runs with header attribution.

Do not delete scientific legacy history.

## Administrative audit begins here

Create an append-only administrative audit table. It is separate from scientific provenance: `record_provenance` (`crates/app/migrations/0007_evidence_identity.sql`), `ai_opinion_exposures` (`0049`) and `ai_screening_audit_labels` (`0054`) keep their meaning and receive no administrative events.

Conceptual table:

```sql
admin_audit_events (
    id uuid primary key,
    occurred_at timestamptz not null,
    event_type text not null,
    principal_kind text not null,
    principal_user_id uuid references users(id),
    target_user_id uuid references users(id),
    session_ref bytea,
    request_id text,
    metadata jsonb not null default '{}'
)
```

- `principal_kind` is `user` or `system`. `system` covers webhook-driven changes such as deactivation. `service` joins with 11 Part A.
- `session_ref` is a keyed hash of the session identifier, never the cookie value.
- `request_id` is the correlation ID (`x-correlation-id`, `crates/http-api/src/routes/mod.rs`), or the inbox receipt ID for webhook events.
- `metadata` takes keys from a closed list per event type. It holds no email, display name, free text or provider payload.
- Foreign keys take no cascade. This handoff never deletes user rows, so audit rows keep their `UserId`.
- A `BEFORE UPDATE OR DELETE` trigger refuses both, following `reject_ai_opinion_exposure_changes` in `0049`. `0049` allows DELETE so that cascades work. This table has no cascades, so it refuses DELETE too. Retention and the one audited deletion path belong to 11, which also owns query access.
- Each row is written in the same transaction as the change it records.

Recorded here:

```text
auth.login
auth.logout
auth.session_revoked
user.provisioned
user.deactivated
```

`user.deactivated` is added to 11 Part B's candidate list. Account deactivation is an account change the audit must show.

Per-request rejections (expired or tampered session, failed origin check, unknown user) go to tracing with the correlation ID, not to this table. Writing each one would let any caller fill an append-only table. `authorization.denied` belongs to 03b and is bounded the same way.

Never store session cookies, access or refresh tokens, the raw session identifier, the `Authorization` header, webhook secrets or raw provider payloads.

## User deactivation

Deactivation is global: it ends the DeepRef user in every workspace. Workspace-scoped membership revocation is 03b.

The webhook path waits for the 04a inbox. The session check does not.

Source: WorkOS `user.deleted`, received through the 04a inbox. Confirm the event name in the WorkOS event reference before implementation. Do not use `dsync.user.deleted`, which is Directory Sync. No WorkOS suspension event is confirmed. If WorkOS exposes none, 04a's reconciliation rules must find suspended users; confirm this before implementation.

`users.status` takes `active` or `deactivated`. In one transaction:

- The inbox verifies the signature and records the delivery before processing (04a), and dedupes on the provider event ID.
- Status changes by compare-and-set from `active` to `deactivated`. A replayed delivery changes nothing.
- No webhook reactivates a user. A later `user.updated` for a deactivated user changes no status. Reactivation is an explicit administrative action outside this handoff.
- `user.deactivated` is written to `admin_audit_events` in the same transaction.

Sessions:

- Session validation reads `users.status` by primary key on every authenticated request. No cache sits in front of it, so a deactivated user's next request fails.
- WorkOS session revocation is requested as best effort. Rejection never depends on it.

Identity:

- Deactivation sets `status`, not `deleted_at`. The row stays, and `users.id` remains the reference that provenance, assignments, usage and audit rows hold.
- Deactivation does not clear `primary_email` or `display_name`. Removing PII follows the LGPD policy gate in `00_PROGRAM.md` and runs as a separate, audited step.
- A new WorkOS user ID is a new `UserId`. Email never re-links an account.

## Frontend

Add the minimum session UX:

```text
sign in
sign out
session expired
current user bootstrap
```

Do not redesign navigation.

Frontend route guards are user experience only. The server remains authoritative.

## Error handling

Distinguish:

```text
unauthenticated
invalid/expired session (a deactivated account is rejected the same way)
origin rejected (403, cookie-authenticated unsafe request)
provider unavailable during login
internal provisioning failure
```

Do not leak provider token details.

## Out of scope

Do not implement:

- workspace membership;
- project authorization;
- WorkOS Organizations;
- Polar;
- API keys;
- NATS;
- scientific assignment redesign;
- replacement of header-based scientific attribution (03c);
- workspace membership revocation and first-workspace onboarding (03a, 03b);
- audit queries, retention and coverage beyond the events listed above (11 Part B).

## Tests

Cover:

- one WorkOS identity maps to one `UserId`;
- duplicate callback/provisioning is idempotent and writes one `user.provisioned` row;
- valid session;
- expired session;
- tampered session;
- logout is `POST` only, and a `GET` to the logout path does not sign out;
- unsafe return URL rejection;
- callback with a missing or mismatched `state` is rejected;
- browser actor headers do not override `UserId`: a request carrying any `x-actor-id` or `x-actor-kind` authenticates, and is audited, as the same `UserId`; an unauthenticated request carrying `x-actor-kind: user` and another person's `x-actor-id` gets 401;
- frontend does not persist credentials or identity in browser storage, and auth, session and `/me` requests send no `x-actor-*` header;
- origin attack tests on a cookie-authenticated `POST`:
  - cross-origin `POST` is rejected with 403 and changes no state;
  - `Origin: null` is rejected;
  - an `Origin` differing only by scheme or port is rejected;
  - a sibling subdomain (same site, different origin) is rejected;
  - missing `Origin` with `Sec-Fetch-Site: same-origin` is allowed; with `same-site`, `cross-site` or `none` it is rejected;
  - missing `Origin` and `Sec-Fetch-Site` is rejected;
  - same-origin `POST` is allowed;
  - an unsafe request without the session cookie is not affected by the origin rule;
- administrative audit: `auth.login`, `auth.logout`, `auth.session_revoked`, `user.provisioned` and `user.deactivated` each write one row in the same transaction as their change; no row contains a cookie value, token or raw session identifier; `UPDATE` and `DELETE` on `admin_audit_events` are refused;
- user deactivation: a `user.deleted` delivery sets `status` to `deactivated` and the user's next request is rejected; a replayed delivery is a no-op; a later `user.updated` does not reactivate; scientific and audit rows keep the same `UserId`.

## Completion criterion

Every authenticated browser request can resolve one stable DeepRef `UserId`; unauthenticated users cannot access protected application state; no header or browser storage value determines authentication, authorization or audit identity; cookie-authenticated unsafe requests pass the origin rule or are rejected; administrative audit records the five identity events without secrets and cannot be altered; a deactivated user's sessions fail on their next request. Legacy scientific attribution from `x-actor-id` remains until 03c, which ships inside the 03 release gate.
