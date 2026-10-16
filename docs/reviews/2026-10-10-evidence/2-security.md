# Review 2: Security and privacy

Reviewer lens: deployment exposure, cost exhaustion, uploads and PDF parsing, SSRF, assistant tool authority, the workflow engine (webhooks, inbound e-mail, HTTP, Slack, e-mail and secret nodes), XSS, supply chain, and data deletion and retention.
Method: static reading of `crates/http-api`, `crates/application/src/workflows`, `services/worker/src/workflows`, `crates/documents`, `crates/ai`, `crates/app`, `apps/web`, the Docker, Caddy and compose files, and `.github/workflows`. Nothing was run against a live server. Each finding cites file:line. A finding is labelled **(inferred)** when it rests on code reading of an exploit path that I did not test end to end.

Baseline the lead already confirmed (not re-derived here): actor identity is self-asserted through the `x-actor-kind`/`x-actor-id` headers (`crates/http-api/src/routes/actor.rs`, defaulting to `local-user`) and through dedup body fields (`deduplication.rs:337`, defaulting to `web-user`). The web app keeps the reviewer id per tab in sessionStorage (`apps/web/src/lib/api/reviewer.ts`). **There is no authentication or authorization anywhere in the API.** Every finding below is made worse by that.

---

## 0. Severity summary

| # | Finding | Severity |
|---|---|---|
| S1 | API has no auth, binds `0.0.0.0` in containers, has `staging`/`production` modes, and published images have no "trusted network only" guard or docs | **Critical** (as soon as it leaves localhost) |
| S2 | DNS rebinding against the local API: no Host allow-list, so any web page the user visits can drive `127.0.0.1:8080` | **High** |
| S3 | Workflow HTTP node: the stored `Authorization` secret goes to whatever URL the node has at run time. The URL can be edited by anyone, or built from webhook/e-mail data through templates | **High** |
| S4 | AI cost: budget is per project, $5 by default, settable up to $100k by any caller; projects are free to create; check-then-act lets concurrent calls overspend | **High** (hosted) / Medium (local) |
| S5 | Crawl and Crossref abuse: `default_max_depth`, `max_depth`, `rate_limit_per_second` and `max_concurrency` have no upper bound | Medium |
| S6 | Uploaded PDFs are never deleted from disk: not on document delete, not on project delete | Medium (privacy, disk) |
| S7 | Assistant plans can bulk-screen up to 5,000 reports under the *human* actor, and confirmation re-reads `revision` at run time (no optimistic concurrency) | Medium |
| S8 | Self-asserted identity defeats the AI-first blinding and fresh-auditor controls. Clients can also claim `automation`/`system` and forge provenance | Medium (research integrity) |
| S9 | Inbound e-mail hook has no provider signature check. Webhook signing is optional. The hook rate limit is keyed on an attacker-chosen string | Medium |
| S10 | E-mail node: recipients can come from templates, with no recipient cap → relay via webhook/e-mail triggers | Medium (inferred) |
| S11 | Templates are not encoded for their context (URL, JSON body) | Medium |
| S12 | PDFium (C++) parses untrusted PDFs inside the worker process that holds DB and AI credentials. Only a between-pages time limit; no process isolation | Medium |
| S13 | SSRF block-lists: two diverging copies; both miss NAT64/6to4/IPv4-compatible ranges; the workflow client honours `HTTP(S)_PROXY` | Low |
| S14 | Import parsers (BibTeX/RIS/NBIB/CSV) run synchronously on the async runtime and are not fuzzed | Low |
| S15 | Retention: `domain_events` (with work titles), AI runs and transcripts, usage ledger and jobs are never pruned. Full-text goes to third-party LLM providers | Low/Privacy |
| S16 | Caddy serves the SPA without a CSP; access logs record webhook/e-mail capability tokens | Low |
| S17 | `multipart/form-data` endpoints can be hit by cross-site requests (CSRF), because no custom header or token is required | Low (inferred) |

What is already good (keep it): the assistant markdown renderer (no `{@html}`, link scheme allow-list), CSV formula neutralisation, the remote-PDF fetcher (pinned resolution, no redirects followed blindly, no proxy, `%PDF-` sniff, size cap), the workflow `net::send` pinning (`resolve_to_addrs`, no redirects, 256 KB cap, 20 s timeout), assistant writes that only queue into a confirm-first plan with the project id injected by the host, SHA-pinned GitHub Actions with a policy check, `cargo deny`, `pnpm audit`, `minimumReleaseAge`, the pinned PDFium sha256, non-root containers, Postgres/GROBID bound to 127.0.0.1 in compose, and internal errors hidden from clients.

---

## S1. Deployment exposure: no auth, but built and published for hosting (Critical once exposed)

Evidence:
- `crates/http-api/src/config.rs:17-18`: `API_BIND_ADDR` defaults to `0.0.0.0:8080`. Only `.env.example:3` and `process-compose.yaml:13` override it to `127.0.0.1`.
- `crates/config/src/environment.rs:7-12`: `Local | Development | Staging | Production`. `config.rs:36-38` requires `API_CORS_ORIGINS` for a non-local environment. The codebase therefore expects hosted deployments, yet nothing requires an auth layer.
- `.github/workflows/publish-images.yml` publishes `deepref-server` (serve/worker) and the web image to `ghcr.io/<owner>` on `v*` tags. `docker/rust-service.Dockerfile:150-151` runs `serve` with the 0.0.0.0 default.
- `apps/web/Caddyfile`: `:8080` reverse-proxies `/api/*` straight to `deepref-api:8080`. The web image is a complete, unauthenticated public front end.
- CORS (`routes/mod.rs:257`) does not protect anything here. It only limits what a *browser* can read, and curl ignores it.
- No doc (`README.md`, `docs/local-development.md`, `docs/deployment-recalibration.md`) says "do not expose this to a network".

Impact once reachable: read every project, document and protocol; delete projects irreversibly (`DELETE /projects/{id}`, `projects.rs:215`); read workflow webhook secrets and capability tokens (`workflows.rs:392-412` returns `webhook_secret`); run workflows that make outbound HTTP calls and send e-mail (S3, S10); spend the AI key (S4); read the operator's `crossref_mailto` (`settings.rs`).

Fix (in order):
1. Make the server refuse to start when `APP_ENV != local` and no auth is configured. At minimum require a static bearer token (`DEEPREF_API_TOKEN`) checked by a tower layer on every route except `/health` and `/hooks/*`. Better: put it behind an OIDC/forward-auth proxy and derive the actor from the verified identity, not from headers.
2. Default `API_BIND_ADDR` to `127.0.0.1:8080` in code. The container sets `0.0.0.0` explicitly.
3. Add a "Security model: single user, trusted machine" section to the README and the image descriptions.

## S2. DNS rebinding against the local API (High)

Evidence: there is no `Host` validation anywhere (`grep header::HOST` finds nothing in `crates/http-api/src`). The API listens on `127.0.0.1:8080` with no auth. Vite ≥6 checks `allowedHosts` for its dev server, but the API port is reached directly as well.

Scenario (inferred, standard technique): the user visits `evil.example`. The attacker's DNS rebinds `evil.example` to `127.0.0.1`. That page's JS then makes same-origin requests to `http://evil.example:8080/...`, so CORS never applies. It can read every project, drive the assistant (cost), read workflow secrets, create a workflow with an HTTP node that exfiltrates project data, and delete projects.

Fix: a tower layer that rejects requests whose `Host` is not in an allow-list (`localhost`, `127.0.0.1`, `[::1]`, plus configured hosted origins). It is cheap, and it also closes most of S17. The S1 token fixes this as well.

## S3. Workflow HTTP node: stored secrets follow an editable or templated URL (High)

Evidence:
- `services/worker/src/workflows/nodes.rs:340-372`: `url = render_template(cfg "url", &data)`. If a secret `auth` exists for `(workflow_id, node_id)`, it is attached as `Authorization`.
- `crates/app/src/workflows.rs:276-310` and `:383-394`: secrets are keyed by `(workflow_id, node_id, key)` only. Saving a new graph keeps the secret for any node id that still exists, *whatever the node's URL or type is now*. Secrets are stored in plaintext (`0028_workflow_engine.sql:152-159`).
- `crates/application/src/workflows/validate.rs:220-222`: URL checks are skipped when the URL contains `{{`.
- `crates/application/src/workflows/conditions.rs:191-211`: `render_template` splices raw text, with no URL encoding.

Exploits:
1. (No auth, so anyone.) PATCH the workflow graph and change the existing HTTP node's URL to `https://attacker.example/`, keeping the node id. Trigger a run. The stored API token arrives at the attacker.
2. (Template-derived host, inferred.) A node configured as `https://api.vendor.com{{data.path}}` and triggered by a webhook or inbound e-mail. If the sender sets `path = ".attacker.example/x"`, the host becomes `api.vendor.com.attacker.example`. Note that `@` is caught by the credentials check in `preflight.rs:76`, but this case is not.

Fix: bind each secret to the origin (scheme+host+port) it was entered for, and drop or re-require it whenever the node's origin changes. Forbid `{{` before the path starts: parse the template with placeholders replaced by a sentinel and require a fixed origin. Percent-encode substituted values in the URL. Encrypt secrets at rest with a key from the environment (low priority while there is no auth).

## S4. AI cost exhaustion (High hosted / Medium local)

Evidence:
- `crates/ai/src/usage.rs:72-80`: `ensure_budget` reads the budget, then the call happens, then the cost is recorded. This is a check-then-act race: N concurrent assistant runs or workflow AI steps all pass the check and then all spend.
- Budget is per project (`0027_ai_usage_and_assistant_plans.sql:4-7`, default $5/month). Projects can be created freely (`POST /projects`), so the cap is per attacker-created project, i.e. unlimited.
- `crates/app/src/ai_usage.rs:52-58`: any caller can set the budget to anything up to `100_000_000_000` micros ($100,000) through `PUT /projects/{id}/ai/budget`.
- Calls with `project_id: None` are "recorded but not budget-limited" (`0027…sql:8-9`, `openai_compat.rs:663`).
- Per-run guards are good: assistant `max_steps: 8`, `max_total_tokens: 60_000` (`runtime/plan.rs:63-73`), `MAX_ASSISTANT_MESSAGE_CHARS` (`assistant/chat.rs:1140`). Unknown models are priced conservatively (`pricing.rs:49-52`).

Fix: add a workspace-wide monthly cap (env, e.g. `DEEPREF_AI_MONTHLY_CAP_USD`) enforced in the same `MeteredGateway`. Reserve estimated cost atomically before the call (an `UPDATE … SET reserved = reserved + $est WHERE spent+reserved+$est <= budget RETURNING`) and settle afterwards. Cap the per-project budget below the workspace cap. Budget raises should be an operator action, not an anonymous one.

## S5. Unbounded crawl and provider-abuse knobs (Medium)

Evidence: `routes/settings.rs:67-87` validates only lower bounds (`reject_negative`, `reject_less_than_one`) for `default_max_depth`, `max_concurrency`, `rate_limit_per_second` and `retry_attempts`. `routes/projects.rs:50` and `routes/acquisitions.rs:183-186` check only `max_depth >= 0`. The worker clamps only `.max(1)` (`services/worker/src/store.rs:40`). I found no item or fan-out budget per ingestion.

Impact: `max_depth = 10` on a well-cited DOI can grow the citation graph exponentially (millions of Crossref calls). `rate_limit_per_second = 100000` hammers Crossref under the operator's `mailto`, which gets the operator's IP and e-mail rate-limited or blocked from the polite pool.

Fix: add upper bounds (for example depth ≤ 3, rate ≤ 50/s per Crossref etiquette, concurrency ≤ 16, retries ≤ 10), a DB `CHECK` to match, and a per-ingestion node budget.

## S6. Uploaded and fetched PDFs are never deleted (Medium, privacy and disk)

Evidence: `crates/documents/src/lib.rs:278-283`: every upload gets a fresh `documents/<uuid>` object key. Nothing in `crates/documents`, `crates/http-api` or `services/worker` deletes a stored object (`remove_file` appears only on the temporary parse copy in `processor.rs:1784` and in tests). `project_deletion.rs:10-38` deletes rows only. There is no document delete endpoint (`routes/documents.rs` routes are read and upload only).

Impact: "Project and everything that belongs to it deleted" (`projects.rs:210`) is false for the most sensitive data (confidential manuscripts, licensed full text). Disk usage only grows.

Fix: on project delete, collect `object_key`s in the transaction, delete the blobs after commit (or via a `blob_gc` job), and add an orphan sweeper that compares the store with `documents.object_key`. Correct the API doc string until then.

## S7. Assistant plan authority and bulk screening (Medium)

Good: writes are queued into a plan. The host overwrites `project_id` (`crates/ai/src/runtime/tools.rs:216-224`, `:266-300`), and nothing runs until `confirm_plan` (`routes/assistant/plans.rs:239`).

Issues:
- `runtime/plan/policy.rs:23-24`: up to 25 actions × 200 report ids = **5,000 screening decisions per confirmation**. Prompt-injected abstract or full-text content can steer the model to propose a mass exclusion (inferred). The user sees one card per action, not 200 titles.
- `plans.rs:631-700` (`run_screen_reports`): decisions are recorded with the *confirming human's* actor (`actor_kind = 'user'`). The only marker is a `notes` suffix, "via assistant (plan …)". Queries that count human decisions (`ai_reviewer.rs:118`, `:271`; `ai_first_fresh_auditor` in `0055_ai_first_blind_controls.sql`) will count these model-proposed decisions as independent human screening.
- `plans.rs:679-696`: `expected_revision` is read from `screening_state` *at execution time*. That defeats optimistic concurrency, so a confirmed plan silently overwrites a decision a co-reviewer made after the plan was drafted.

Fix: capture `revision` per report when the plan is drafted and use it at confirm time (report stale entries as "skipped: changed since proposed"). Record assistant-originated decisions with distinct provenance (a column or an `actor_kind`), and exclude them from human-agreement and independence calculations. Lower the bulk cap or show every affected title before confirming.

## S8. Self-asserted identity breaks blinding and provenance (Medium, research integrity)

Building on the confirmed self-asserted headers:
- `actor.rs:22-23` accepts `x-actor-kind: user | automation | system`. Any client can write audit-trail entries as `automation` with any id, e.g. `ai:<model>` (`ai_autonomy_apply.rs:32`), `second-reviewer` (`second_review.rs:200`) or `ai-first-routing`. Provenance in exports (`audit_export.rs`) then lies.
- `ai_first_fresh_auditor` and the exposure checks (`ai_first/audit.rs:267-276`, `0055…sql`) decide blinding by `actor_id`. A reviewer who has seen the AI opinion can switch `x-actor-id` (open a new tab: the id lives in per-tab sessionStorage) and count as a fresh, unexposed auditor. The audit stratum's validity guarantee depends on identities that are not authenticated. The code does acknowledge this (`"actor_identity":"self_asserted"`, `audit_export.rs:616`). The UI and exported claims should say so prominently, or the feature should be gated on authenticated identity.

Fix: reject `automation`/`system` from HTTP clients (server-side code constructs those actors itself). Once real auth exists, derive `x-actor-id` from it.

## S9. Hook endpoints (Medium)

Evidence (`crates/http-api/src/routes/workflows.rs`):
- `:1709-1729` inbound e-mail: authorization is only the 128-bit path token (`security.rs:60-62`). There is no Mailgun HMAC (`timestamp/token/signature`) or Postmark basic-auth verification, so anyone who learns the address can inject e-mails. The address is also visible to anyone via GET `EndpointsDto` (`:392-412`) while there is no auth.
- `:1612-1621`: webhook HMAC is checked only when `signature_required` is true. Unsigned webhooks accept any body.
- `:1473-1492` `hook_allowed`: the rate limit is keyed by the *path token string, before lookup*, so random tokens each get a fresh window and a DB lookup. The map is pruned only when `len > 10_000`, and then only of entries older than 60 s, so it can grow without bound within a minute. It is also per process and fails open on a poisoned mutex (`return true`).
- Capability tokens sit in the URL path, and Caddy's JSON access log (`apps/web/Caddyfile` `log { output stdout }`) records `request.uri`, so tokens end up in logs.

Fix: verify the provider signature for inbound e-mail (configurable per provider). Default `signature_required = true` for new webhooks. Rate-limit by client IP *and* only for known tokens, after lookup, using a bounded LRU. Redact `/hooks/*` paths in Caddy and Tower logs.

## S10. E-mail node can become a relay (Medium, inferred)

Evidence: `services/worker/src/workflows/nodes.rs:280-300`: recipients come from `render_template(cfg "to", &data)` split on commas, with no count limit. `preflight.rs:147-155` checks only syntax. `validate.rs:254` skips checks when `to` contains `{{`.

Scenario: a workflow with an inbound e-mail or webhook trigger and `to = {{data.from}}` (a natural "auto-reply" setup). An external sender can then direct the operator's SMTP identity at arbitrary or many addresses, with an attacker-influenced subject and body via other `{{}}` fields. Even without templating, the absence of auth lets anyone configure a spam workflow.

Fix: cap recipients (for example 10). Optionally restrict templated recipients to an allow-listed domain set in env. Rate-limit e-mail sends per workflow.

## S11. Templates are not encoded for their context (Medium)

`render_template` (`conditions.rs:191-211`) substitutes raw text into URLs (S3), JSON bodies (`nodes.rs:343`) and the Slack JSON (`nodes.rs:322`, which is safe because it goes through `json!`). In the HTTP body, a record title containing `"` breaks or alters the JSON structure (JSON injection into the downstream API). Fix: render with an escaper chosen per field kind (URL component, JSON string, plain text).

## S12. PDF parsing isolation (Medium)

Evidence: `crates/documents/src/parser.rs:117-138`: PDFium (C++, via `pdfium-render`) runs in the worker process under a process-wide `PARSE_LOCK`, with limits of 500 pages, 50k blocks, 10 MiB text and `max_duration`. The duration is checked **only between pages**. `services/worker/src/processor.rs:1783` wraps `spawn_blocking` with no `tokio::time::timeout`. Uploads are limited to 25 MiB and sniffed for `%PDF-` (`documents.rs:602`, `lib.rs:31`), and the binary is pinned by sha256 (`rust-service.Dockerfile:107-112`).

Impact: one pathological page (or a PDFium hang) holds `PARSE_LOCK` forever and stalls every document parse. A memory-safety bug in PDFium runs inside the process that holds `DATABASE_URL` and the AI API key.

Fix: run PDFium in a short-lived subprocess (`deepref-server parse-pdf <path>`) with rlimits (CPU, AS), a wall-clock kill, no network, and the output read as JSON. Alternatively, at minimum, a watchdog that restarts the worker. Keep PDFium on a schedule for updates (Chromium CVEs).

## S13. SSRF block-lists (Low)

Good: `services/worker/src/workflows/net.rs:27-118` resolves once, pins the connection with `resolve_to_addrs`, follows no redirects, and has a timeout and a body cap. `crates/documents/src/remote.rs:150-230` additionally re-checks `remote_addr()` and sets `.no_proxy()`.

Gaps:
- Two independent lists: `application/workflows/preflight.rs:27-50` and `documents/remote.rs:326-359`. They already differ (remote.rs covers `2001::/23`-ish and `3fff::/20`, preflight.rs does not). Neither covers NAT64 `64:ff9b::/96` (it reaches IPv4 internals on NAT64 networks), 6to4 `2002::/16` with an embedded private v4, IPv4-compatible `::a.b.c.d`, `198.18.0.0/15`, `240.0.0.0/4` or `192.0.0.0/24`.
- `net::send` does not call `.no_proxy()`. If the worker runs with `HTTP(S)_PROXY` set, reqwest sends through the proxy and the proxy re-resolves the name, which bypasses both the pinning and the check.

Fix: one shared `is_public_ip` in a small crate, extended with the ranges above and unit-tested against an RFC 6890 table. Add `.no_proxy()` to `net::send`.

## S14. Import parsers (Low)

`routes/acquisitions.rs:438-441`: the import is parsed synchronously inside the async handler (`MAX_IMPORT_BYTES` 2 MiB, body cap 12 MiB+), which blocks a Tokio worker thread for the whole parse. `crates/providers/src/imports.rs` (biblatex 0.12, csv, hand-written RIS/NBIB) has no fuzz target. `fuzz/fuzz_targets` covers only DOI, title normalisation and the screening parser. Fix: `spawn_blocking` plus a timeout, a record-count cap, and a `fuzz_import_parsers` target per format.

## S15. Retention and third-party data flow (Low / privacy)

- Only notifications are pruned (`crates/app/src/notifications.rs:7-10`). `domain_events` (payloads include `WorkUpserted.title`, `crates/events/src/domain.rs:26-31`), `jobs`, AI runs, assistant transcripts, `ai_usage_ledger` and `workflow_runs` grow forever. Global `works` rows created from a private import survive project deletion.
- Full text, abstracts and protocols are sent to the configured provider (default `https://opencode.ai/zen/go/v1`, routing to GLM, Kimi or DeepSeek; `crates/config/src/ai.rs:7-9`). For confidential manuscripts or embargoed data, users need a visible disclosure and a per-project "no AI" switch.

Fix: a documented retention policy plus a periodic purge job (completed jobs older than N days, domain_events after projection, AI transcripts per project setting). Show provider and data-flow disclosure in Settings.

## S16. Web hardening (Low)

`apps/web/Caddyfile` sets nosniff, `X-Frame-Options: DENY` and Referrer-Policy, but has **no Content-Security-Policy**. XSS sinks are already well controlled: there is no `{@html}` or `innerHTML` in `apps/web` or `packages/ui`, the assistant markdown is parsed to a whitelisted node tree with `http`, `https`, `mailto` and same-app links only (`features/assistant/markdown.ts:85-106`), and DOI links are encoded (`features/full-text/structure.ts:56`). A CSP (`default-src 'self'; script-src 'self'; object-src 'none'; frame-ancestors 'none'; connect-src 'self'`, plus whatever SvelteKit inline bootstrapping needs via hash) would make this hold by default. Inline PDF responses set `Cache-Control: private, no-store` and a sanitised filename (`documents.rs:570-582`, `sanitize_filename` at `:1188`), which is fine.

## S17. Cross-site "simple" requests (Low, inferred)

JSON endpoints need `Content-Type: application/json`, which forces a CORS preflight, so they are safe from classic CSRF. But `multipart/form-data` (document upload, `documents.rs:639`) and `application/x-www-form-urlencoded`/multipart (inbound e-mail) are "simple" requests. The actor headers are optional and default to `local-user`. A malicious page can therefore POST a PDF into a known project/report id on `localhost:8080` without a preflight. The impact is limited because ids are UUIDs that the attacker must know. The S2 Host check or a required custom header (e.g. `X-Requested-With`) on non-hook routes closes it.

---

## Supply chain (no findings of note)

- All GitHub Actions are pinned to commit SHAs with version comments, and `security.yml:84-106` enforces this. `publish-images.yml` has least-privilege `permissions` (`contents: read`, `packages: write`).
- `cargo deny --locked check`, `cargo shear`, `pnpm audit --audit-level high` and CodeQL run in CI (`security.yml`). pnpm uses `onlyBuiltDependencies`, `minimumReleaseAge` and `patchedDependencies` (`pnpm-workspace.yaml`).
- Base images are pinned by digest (`apps/web/Dockerfile`). PDFium is downloaded from `bblanchon/pdfium-binaries` and verified by a per-architecture sha256.
- Minor: compose uses `POSTGRES_PASSWORD: postgres` (`compose.yaml:9`), which is acceptable because it is bound to 127.0.0.1. Nothing stops someone reusing that compose file on a server.

## Suggested order of work

1. S1 + S2 together: a startup guard for non-local environments, a bearer token or forward-auth, a Host allow-list layer, and a `127.0.0.1` code default. This removes the anonymous variants of S3, S4, S5, S8, S9, S10 and S17 at once.
2. S3: bind secrets to origin; fixed-origin URL templates.
3. S4: a workspace-wide cap and atomic reservation.
4. S6: blob deletion and GC (users are told it already happens).
5. S7: revision captured at plan time, distinct provenance for assistant-originated decisions.
6. S12: PDFium subprocess.
7. The rest as hardening.
