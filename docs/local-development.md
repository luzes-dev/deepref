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

Local endpoints:

| Service         | Address                 |
| --------------- | ----------------------- |
| Web             | `http://127.0.0.1:5173` |
| API             | `http://127.0.0.1:8080` |
| PostgreSQL      | `127.0.0.1:5432`        |

If port `5432` is already in use, set `POSTGRES_HOST_PORT` and update the
port in `DATABASE_URL` in `.env` to the same value before starting the stack.

Every published dependency port binds only to `127.0.0.1`.

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

| Variable | Default | Meaning |
| --- | --- | --- |
| `DEEPREF_AI_PROVIDER` | `opencode-go` | `opencode-go`, or `zai` for the retired legacy provider |
| `OPENCODE_API_KEY` | none | Required for OpenCode Go. Without it AI features are off |
| `OPENCODE_BASE_URL` | `https://opencode.ai/zen/go/v1` | Endpoint override |
| `DEEPREF_AI_DEFAULT_MODEL` | `glm-5.3-flash` | Model for new AI routes |
| `DEEPREF_AI_MODEL_PRICES` | built-in | Price overrides: `provider/model=input:output`, US$ per million tokens |

**Limits and budget.** OpenCode Go caps each model in dollars: a 5-hour window (20% of the monthly cap), a weekly window (50%) and the monthly cap. When a cap is spent, DeepRef stops that call without retrying and reports "AI subscription limit reached; try again later". The public docs do not give the provider's error text, so DeepRef matches the usual wording for usage, spending, weekly and monthly limits.

**Budget is a soft guard.** A subscription has no per-token bill. Each project's monthly budget is therefore counted in US dollars, at the published per-token rate for each model (the same numbers OpenCode lists for its pay-as-you-go plan). The usage ledger records every call, streamed or not. It is an estimate for guarding spend, not an invoice.

**Legacy Z.AI.** Z.AI is retired. `ZAI_API_KEY` and `ZAI_BASE_URL` are ignored unless `DEEPREF_AI_PROVIDER=zai`, and then DeepRef logs a deprecation warning at startup. Routes created under the old `zai` label are re-pointed to the configured provider when it changes, so model identity and calibration always name the real endpoint. Past runs and ledger rows keep their original label.

## Seed data

In another terminal, run:

```bash
mise exec -- just seed
```

The seed is deterministic and idempotent. It creates one project with three works and three citation edges. Its Crossref address is a reserved placeholder; set a real contact address in application settings before starting an actual ingestion.

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

Create a feature branch only from a clean worktree:

```bash
mise exec -- just feature my-change
```

The recipe fetches and fast-forwards `main` before creating `feature/my-change`.
