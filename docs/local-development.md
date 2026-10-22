# Local development

DeepRef uses mise for pinned tools, Just for the developer command surface, Docker Compose for disposable dependencies, and Process Compose for application processes.

The local PostgreSQL service uses the pinned `pgvector/pgvector:0.8.7-pg17-bookworm`
image. This is required because migration `0016_ai_foundation.sql` installs
the `vector` extension; use the same pgvector-enabled PostgreSQL 17 image for
disposable migration/integration fixtures.

## Prerequisites

Install:

- mise
- a Docker-compatible engine with the `docker compose` plugin

From the repository root:

```bash
mise trust
mise install
mise exec -- just bootstrap
```

`bootstrap` creates `.env` from `.env.example` only when `.env` is absent, installs locked JavaScript and Rust dependencies, and installs the Playwright Chromium build.

## Run the stack

```bash
mise exec -- just dev
```

`just dev` waits for PostgreSQL to become healthy, applies PostgreSQL migrations, and then starts the web app, API, and worker under Process Compose. Process names prefix their logs. API readiness gates the other processes, and Ctrl-C sends each service its configured clean shutdown signal in reverse dependency order.

Workers spend an attempt when a lease expires: recovery backs off retries and dead-letters jobs at their attempt limit. Queue and workflow writes require the current job ID, owner, and attempt with an unexpired lease; restarting the same worker does not authorize a stale attempt. Transient renewal errors retry within the lease deadline, and a stalled renewal is cancelled at that deadline. Handler panics fail the individual job, including panics while constructing its future, rather than stopping the claim loop. `/health/dependencies` reports live worker heartbeat rows; a clean shutdown removes the row after draining.

Local endpoints:

| Service    | Address                 |
| ---------- | ----------------------- |
| Web        | `http://127.0.0.1:5173` |
| API        | `http://127.0.0.1:8080` |
| PostgreSQL | `127.0.0.1:5432`        |

If port `5432` is already in use, set `POSTGRES_HOST_PORT` and update the
port in `DATABASE_URL` in `.env` to the same value before starting the stack.

Every published dependency port binds only to `127.0.0.1`.

`just` recipes and Process Compose run with `APP_ENV=development`. The server binary itself
refuses to start when `APP_ENV` is unset, and `local` and `staging` are rejected: the only
environments are `development` and `production`. It reads `.env` and `.env.local` only when
the process environment already says `APP_ENV=development`; production never reads dotenv
files. To run it by hand, export that first, for example
`APP_ENV=development cargo run -p deepref-server -- serve`.

`DEEPREF_PUBLIC_ORIGIN` is the canonical origin users reach the app at (scheme, host and
optional port, no path). Development defaults to `http://127.0.0.1:5173`; production
requires it and rejects anything but `https`.

API `/health/ready` requires the latest embedded SQLx migration, not a fixed
schema version. An older database returns 503; a newer database remains ready
and emits a warning. Set `DEEPREF_BUILD_SHA` to the deployed revision to populate
the startup log and OpenTelemetry `service.version` (otherwise `unknown`).

Send a UUID in `x-correlation-id` to correlate API response headers, error bodies,
and INFO-level request spans. Invalid or missing IDs are replaced once per
request. Database failures never expose SQL or constraint names: retryable
failures return 503 with `Retry-After: 1`, constraint conflicts return 409, and
unexpected failures return a sanitized 500. Metric recomputation retries only
whole transactions rolled back for serialization failures or deadlocks; it
does not retry ambiguous connection/commit failures.

## Fast continuous Rust feedback (Bacon)

Bacon is pinned in `.mise.toml` as an interactive local developer tool (it is not a CI gate). It watches the codebase and runs checks or tests on file changes:

```bash
# Continuous compilation check (default)
mise exec -- bacon

# Continuous Clippy linting
mise exec -- bacon clippy

# Continuous unit test execution with nextest
mise exec -- bacon nextest

# Continuous AI module test execution
mise exec -- bacon ai
```

While running interactively in the terminal, keyboard shortcuts allow instant switching:

- `c`: switch to `clippy`
- `n`: switch to `nextest`
- `a`: switch to `ai`
- `k`: switch to `check`
- `d`: generate and browse Rust documentation

## AI provider

DeepRef's AI runs on the **OpenCode Go** subscription through its OpenAI-compatible chat-completions API. The default model is `glm-5.3-flash`.

1. Subscribe in the OpenCode console and copy the API key.
2. Put it in `.env.local` (gitignored), never in `.env`:

   ```bash
   OPENCODE_API_KEY=<your key>
   ```

3. Check the key and the model list. This call does not run inference:

   ```bash
   curl -s -H "Authorization: Bearer $OPENCODE_API_KEY" https://opencode.ai/zen/go/v1/models | head -c 300
   ```

4. Restart the API and worker. Settings → AI shows the provider as "OpenCode Go" and the model.

Settings:

| Variable                   | Default                         | Meaning                                                                |
| -------------------------- | ------------------------------- | ---------------------------------------------------------------------- |
| `DEEPREF_AI_PROVIDER`      | `opencode-go`                   | `opencode-go`, or `zai` for the retired legacy provider                |
| `OPENCODE_API_KEY`         | none                            | Required for OpenCode Go. Without it AI features are off               |
| `OPENCODE_BASE_URL`        | `https://opencode.ai/zen/go/v1` | Endpoint override                                                      |
| `DEEPREF_AI_DEFAULT_MODEL` | `glm-5.3-flash`                 | Model for new AI routes                                                |
| `DEEPREF_AI_MODEL_PRICES`  | built-in                        | Price overrides: `provider/model=input:output`, US$ per million tokens |

Usage export to the billing provider is off by default and provider-neutral until handoff 08 picks one:

| Variable                                 | Default | Meaning                                                              |
| ---------------------------------------- | ------- | -------------------------------------------------------------------- |
| `DEEPREF_USAGE_EXPORT_ENABLED`           | `false` | Emit settled usage facts to the billing provider's usage export sink |
| `DEEPREF_USAGE_EXPORT_REPLAY_BATCH_SIZE` | `500`   | Usage facts one replay batch emits at most (1–1000)                  |

While the export is off nothing is emitted and nothing is lost: settled usage stays in the ledger and a later replay exports it, and the worker registers no export consumer. The emitter also runs on demand as `deepref-server ai-usage export-replay [--since <rfc3339>] [--limit <rows>]`. See [AI usage reservations](ai-usage.md#export) for the event contract, the sink semantics and the runbook for enabling a provider.

**Limits and budget.** OpenCode Go caps each model in dollars: a 5-hour window (20% of the monthly cap), a weekly window (50%) and the monthly cap. When a cap is spent, DeepRef stops that call without retrying and reports "AI subscription limit reached; try again later". The public docs do not give the provider's error text, so DeepRef matches the usual wording for usage, spending, weekly and monthly limits.

**Budget is a soft guard.** A subscription has no per-token bill. Each project's monthly budget is therefore counted in US dollars, at the published per-token rate for each model (the same numbers OpenCode lists for its pay-as-you-go plan). The usage ledger records every call, streamed or not. It is an estimate for guarding spend, not an invoice.

**Legacy Z.AI.** Z.AI is retired. `ZAI_API_KEY` and `ZAI_BASE_URL` are ignored unless `DEEPREF_AI_PROVIDER=zai`, and then DeepRef logs a deprecation warning at startup. Routes created under the old `zai` label are re-pointed to the configured provider when it changes, so model identity and calibration always name the real endpoint. Past runs and ledger rows keep their original label.

## Seed data

In another terminal, run:

```bash
mise exec -- just seed
```

The seed is deterministic and idempotent. It creates one project with three works and three citation edges. Ingestions identify themselves to OpenAlex, Crossref, DataCite, NCBI and Unpaywall with `DEEPREF_CONTACT_EMAIL`, so set that variable in `.env` before running a real import.

Without `DEEPREF_CONTACT_EMAIL`, DOI and PubMed ID import requests return HTTP 503 without creating an acquisition or queuing a job. A worker missing the variable also fails already-queued PubMed items and PMID-only workflow imports permanently, with an error naming the variable, before contacting NCBI. Set the address and restart both the API and worker before submitting the import again.

DOI imports try the providers in `DEEPREF_METADATA_PROVIDERS` in order. The default is `openalex,crossref,datacite`. To roll back to the Crossref-only lane, set `DEEPREF_METADATA_PROVIDERS=crossref` and restart both the API and the worker. The rollback also reaches runs created earlier, because a stored chain keeps only the providers the deployment still enables. Keyless OpenAlex is valid, and `DEEPREF_OPENALEX_API_KEY` is optional. A key that is pending or invalid downgrades OpenAlex to keyless until the process restarts, and imports keep running. Deploy the API and the worker together, so both read the same chain, and deploy the whole routing stack at once: the work cache never expires, so an answer cached by an incomplete build keeps its short reference list. Until the corrected stack is deployed, keep `DEEPREF_METADATA_PROVIDERS=crossref`.

`DEEPREF_OPENALEX_CORPUS` is `core` by default, which looks DOIs up in the core corpus only and treats an expansion record (`is_xpac`) as not found. Set it to `all` to accept expansion records too. Either way the singleton lookup sends the `corpus` parameter, and a 404 is a miss with no paid retry. When Crossref is enabled in the deployment, the worker asks Crossref for its reference list after every OpenAlex answer and adds the DOIs that are missing, because OpenAlex's reference count cannot show a reference it never matched. Entries without a DOI land in the project's unresolved references. The union is not switchable. A work whose references came back degraded (a spent OpenAlex budget, omitted targets, or a failed Crossref list) stays cached with `deepref_reference_status` set to `degraded`, and a reset, a refresh or a new acquisition does not repair it. The daily list budget pause is per process, so each worker replica sees one 429 before it stops. Restart the API and the worker after changing either variable.

## Stop or reset

```bash
mise exec -- just dev-down
```

This stops local processes and containers but retains the PostgreSQL volume. To delete all disposable PostgreSQL data:

```bash
mise exec -- just dev-reset
```

`dev-reset` permanently removes only the named PostgreSQL volume owned by `compose.yaml`.

## Common commands

Run `mise exec -- just --list` for the complete command surface. Common checks are:

```bash
mise exec -- just verify
mise exec -- just test-unit
mise exec -- just test-integration
mise exec -- just test-e2e
mise exec -- just codegen-check
```

With `DATABASE_URL` and `TEST_DATABASE_URL` pointing to a disposable migrated
database, run the focused inbox suite with:

```bash
DEEPREF_REQUIRE_DATABASE=1 cargo nextest run -p deepref-worker --test inbox_processor --locked
```

The inbox lock-compatibility regression holds a real handler transaction open,
checks the inbox row with `FOR UPDATE NOWAIT`, and renews its claim before
releasing the handler. It awaits handler completion rather than inferring it
from effect visibility. Healthy processor fixtures use the production shutdown
budget; only the blocked-drain regression overrides it to 100 ms and asserts
abort, handler cancellation, and rollback. Processor heartbeat and ownership loss
are covered separately. Keep the configured parallel PostgreSQL test group when
running the full integration diagnostics.

Create a feature branch only from a clean worktree:

```bash
mise exec -- just feature my-change
```

The recipe fetches and fast-forwards `main` before creating `feature/my-change`.
