# Security Architecture

DeepRef is a multi-tenant software-as-a-service (SaaS) application designed for systematic literature reviews with reproducible, accountable AI assistance. This document describes the security boundaries, authentication and authorization architecture, tenant isolation, and audit logging.

All production database infrastructure, object storage, and backup systems reside in the São Paulo region (see `docs/saas-program/BUSINESS_DECISIONS.md` for compliance and retention policies).

## Authentication Boundary

User identity and sessions are managed through WorkOS.

- **Configuration:** WorkOS integration is configured via `WORKOS_CLIENT_ID`, `WORKOS_API_KEY`, `WORKOS_COOKIE_PASSWORD`, and `WORKOS_WEBHOOK_SECRET` in `crates/config/src/workos.rs`, gated by `DEEPREF_AUTH_ENABLED`.
- **Fail-fast validation:** In production (`APP_ENV=production`), enabling authentication requires a complete WorkOS configuration. Sealed session encryption requires a cookie password of at least 32 characters (`MIN_COOKIE_PASSWORD_BYTES = 32`).
- **Session management (planned / landing in 02):** Authenticated requests present an encrypted WorkOS session cookie. Session validation checks `users.status` on each request by primary key so that revoked or deactivated accounts cannot make subsequent calls even if their session cookie has not yet expired.
- **User deactivation (planned / landing in 02):** When a user is deleted in WorkOS, an incoming webhook event triggers an atomic compare-and-set transitioning `users.status` to `deactivated`. In the same database transaction, a `user.deactivated` event is written to `admin_audit_events`. Deactivation invalidates active sessions while preserving user references on historical review actions, screening decisions, and provenance records.
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
- **Origin guard (planned):** Mutating state requests verify browser `Origin` and `Sec-Fetch-Site` headers to ensure requests originate from the application's authenticated origin.

## Tenant Isolation (landing in 03b)

DeepRef enforces multi-tenant boundary isolation across all data access:

- **Tenancy model:** Tenancy is organized hierarchically: `User -> WorkspaceMembership -> Workspace -> Project`. Each systematic review project belongs to exactly one workspace.
- **Central authorization (landing in 03b):** Authorization decisions pass through a central Policy Decision Point / Policy Enforcement Point (PDP/PEP) using typed permissions and a route/resource coverage matrix.
- **Cross-workspace protection:** Every route confirms that the authenticated caller holds an active membership and sufficient privileges in the project's owning workspace. Cross-workspace access without membership is rejected.
- **Scientific provenance independence:** Provenance records (`record_provenance`, `ai_opinion_exposures`, `ai_screening_audit_labels`) track scientific contributions separately from administrative roles, preserving review history regardless of subsequent membership changes.

## Incident and Security Audit (planned / landing in 02 and 11 Part B)

Administrative actions, identity events, and security exceptions are recorded in an append-only audit stream:

- **Audit table:** `admin_audit_events` (landing in handoff 02 and expanded in handoff 11 Part B) records security-relevant events, including:
  - Authentication lifecycle: `auth.login`, `auth.logout`, `auth.session_revoked`
  - User identity lifecycle: `user.provisioned`, `user.deactivated`
  - Workspace tenancy (landing in 03a): `workspace.*`, `membership.*` and `invitation.*` events
  - Access denials (landing in 03b): bounded `authorization.denied` events
  - Billing and machine-credential events are added with the billing provider (undecided; see BUSINESS_DECISIONS.md) and with API keys (11 Part A, planned)

  The authoritative list is the `event_type` CHECK on `admin_audit_events` in the migrations.
- **Tamper resistance:** A database trigger blocks updates, deletions and truncation on `admin_audit_events`.
- **Transactional consistency:** Audit entries are committed within the same database transaction as the state change they describe.
- **Volume bounding:** Authorization denial events and authentication failures are rate-limited or deduplicated to protect audit storage from denial-of-service or credential-stuffing floods.
- **Data retention:** Security and access audit logs are retained for 6 months (in compliance with Brazilian Marco Civil da Internet); billing and accounting records are retained for 5 years; point-in-time recovery (PITR) backups are kept for 35 days in São Paulo (see `docs/saas-program/BUSINESS_DECISIONS.md`).
