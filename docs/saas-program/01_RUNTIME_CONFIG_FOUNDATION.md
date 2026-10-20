# Handoff 01: SaaS runtime and configuration foundation

## Goal

Replace the old deployment assumptions with a strict SaaS runtime model and create configuration seams required by WorkOS, Polar, NATS, Valkey and production object storage.

Do not integrate those providers yet.

Read `00_PROGRAM.md` first.

## Scope

Implement:

- `development | production` environment model;
- explicit `APP_ENV`;
- development dotenv loading;
- production fail-fast configuration;
- typed secret handling;
- canonical application origin;
- typed optional infrastructure configuration for WorkOS, Polar, NATS, Valkey and object storage;
- health/readiness configuration boundaries;
- removal of code paths that exist only for the former self-hosted/local deployment model.

## Repository reconnaissance

Before editing, inspect current `main` for:

```text
APP_ENV
Environment
is_local
load_local_dotenv_files
DEEPREF_ALLOW_NETWORK_BIND
API_BIND_ADDR
API_CORS_ORIGINS
RuntimeConfig
process-compose
docker compose
Dockerfiles
deployment docs
architecture-boundary checks
```

Use current names and structures rather than assuming this handoff's examples still match.

## Required environment model

Target:

```rust
pub enum Environment {
    Development,
    Production,
}
```

`APP_ENV` must be explicit. A missing `APP_ENV` is a startup error, not a fallback. It must never default to `Development`; if any default is retained it stays fail-closed (`Production`), preserving the direction of `Environment::from_app_env` in `crates/config/src/environment.rs`, where an unset variable resolves to production today.

Accepted aliases may include:

```text
development | dev
production  | prod
```

if that fits existing conventions.

Do not create `Environment::Test`. Tests inject configuration.

## Development behavior

Development may:

- read `.env.local` and `.env`;
- bind loopback by default;
- use WorkOS development resources;
- use Polar Sandbox;
- connect to local NATS and Valkey;
- use filesystem-backed object storage where current development workflows need it.

Rename helpers that still call this a `local` product environment.

## Production behavior

Production must:

- never read developer dotenv files;
- require a canonical public application origin;
- reject insecure or contradictory cookie/origin configuration;
- require WorkOS configuration once authentication is enabled;
- reject obvious sandbox/test provider credentials once a provider is marked production-enabled;
- use explicit production NATS/Valkey/object-storage endpoints when those components are enabled;
- fail startup rather than silently fall back to development behavior.

Do not implement provider-specific business logic here. Validate configuration shapes only.

## Secrets

Introduce or use a type that prevents accidental secret formatting, such as `secrecy::SecretString`, when it fits the current config architecture.

Candidate secrets include:

```text
WORKOS_API_KEY
WorkOS session/cookie secret
WORKOS_WEBHOOK_SECRET

POLAR_ACCESS_TOKEN
POLAR_WEBHOOK_SECRET

NATS credentials
VALKEY credentials
object-storage credentials
AI/provider keys
SMTP credentials
```

A debug representation of runtime config must not reveal them.

## Infrastructure configuration

Create typed configuration for future adapters without wiring business behavior.

Examples of concepts, not required field names:

```text
WorkOsConfig
PolarConfig
NatsConfig
ValkeyConfig
ObjectStorageConfig
```

NATS and Valkey configuration is optional and inert until `T-NATS` and `T-VALKEY` fire (see 00). Nothing reads it, and its absence is not a startup error.

Configuration should be injectable in tests.

Avoid global environment reads outside `deepref-config`.

## Health model

Separate:

```text
liveness
readiness
dependency health
```

Do not make liveness depend on every external provider.

Readiness may require authoritative dependencies such as PostgreSQL.

NATS, Valkey, WorkOS and Polar health should later appear as dependency status without causing process death for every transient outage.

## Documentation changes in this handoff

Update current developer-facing environment documentation enough that it no longer instructs engineers to use `APP_ENV=local` or `staging`.

Do not perform the full README/product documentation rewrite. That belongs to handoff 11.

## Out of scope

Do not implement:

- WorkOS login;
- workspaces;
- authorization;
- NATS publishing;
- Valkey calls;
- Polar billing;
- object-storage migration;
- CQRS/read models.

## Tests

Cover at minimum:

- missing `APP_ENV` never yields `Development` and still fails closed;
- development config;
- production config;
- production refusing development-only unsafe settings;
- dotenv loaded only in development;
- secrets absent from `Debug`;
- deterministic injected config tests.

Run architecture/configuration test suites and the repository's normal verification gates.

## Completion criterion

The task is complete when the executable runtime has only `development` and `production`, configuration can describe every later infrastructure dependency without coupling domain code to providers, production fails closed on unsafe configuration, a missing `APP_ENV` never resolves to development, and no live executable path depends on the removed `Local` or `Staging` environment variants.
