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

Inspect current `x-actor-id`, `x-actor-kind`, reviewer session storage and related client code.

This handoff must ensure authentication no longer trusts them.

Do not delete scientific legacy history.

The final replacement of live scientific actor attribution happens with handoff 03.

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
invalid/expired session
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
- scientific assignment redesign.

## Tests

Cover:

- one WorkOS identity maps to one `UserId`;
- duplicate callback/provisioning is idempotent;
- valid session;
- expired session;
- tampered session;
- logout;
- unsafe return URL rejection;
- browser actor headers do not override `UserId`;
- frontend does not persist credentials in browser storage.

## Completion criterion

Every authenticated browser request can resolve one stable DeepRef `UserId`; unauthenticated users cannot access protected application state; browser-controlled actor labels no longer function as authentication.
