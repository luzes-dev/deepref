# Agent brief: applying the 2026-10-10 review (wave 1)

**Recovered integration:** wave 1 and approved H2–H5 are integrated on `integration/review-resume`. Read [Integration results](2026-10-10-fix-handoff.md#integration-results-2026-10-14) for per-packet status, final gate counts, actual runtime evidence and unapproved owner gates. The instructions below describe the original track split, not a request to rerun completed work.

Several agents work at the same time, each on one **track**. Read this brief, then your track's prompt, then the parts of the handoff your track names. This brief only covers what is shared or what has changed since the handoff was written.

- **Handoff (your work order):** `/home/luzes/Code/deepref/docs/reviews/2026-10-10-fix-handoff.md`. Follow its "How to work", "Verification gates" and "Environment gotchas" sections, except where this brief overrides them.
- **Review:** `/home/luzes/Code/deepref/docs/reviews/2026-10-10-whole-application-review.md`
- **Evidence:** `/home/luzes/Code/deepref/docs/reviews/2026-10-10-evidence/`

## Base and branch

- **Packet 1 is already implemented** on branch `ci/run-db-integration-tests-with-isolated-databases` (PR #209, head `cc5f3f5`). It adds `deepref_app::test_isolation`: every DB test clones a private database from a migrated template. New DB tests use the same helper as their neighbours.
- **Your worktree starts from that commit.** Create your track's branch from it: `git switch -c <your-branch>`.
- **Agents commit locally; the orchestrator publishes.** The owner authorized reviewed branches and draft PRs stacked on #209 on 2026-10-11; see [Owner decisions](#owner-decisions-2026-10-11-for-wave-2). Attribute commits to their actual authors.

## Shared machine (6 cores, 15 GB RAM, ~37 GB free on /home)

- **Cargo target directory: your worktree's own, never a shared one.** _(Changed 2026-10-12: the old shared target caused stale builds, because workspace crates hash identically across worktrees, so one worktree could link another's crate. Re-run any gate you ran under the shared target.)_
  - Run every cargo command with: `env -u CARGO_TARGET_DIR RUSTC_WRAPPER=sccache CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=2 SQLX_OFFLINE=true cargo …`. _(Changed 2026-10-12: full debug info across ~55 test binaries made each target 40–70 GB and filled the disk. Without debug info and incremental caches, a target is several times smaller.)_. The target dir is then `<worktree>/target`, and sccache shares dependency builds across worktrees.
  - Unset `SQLX_OFFLINE` only for `sqlx prepare`, which runs against your migrated container.
  - The orchestrator owns compilation, formatting, and verification after all track edits are integrated. Agents finish their edits without running checks concurrently.
  - Run focused regressions for integration failures, then the full-workspace gates on the final integrated code.
  - When your track is finished, after the final gates, delete `<worktree>/target` to give the disk back. That directory is the one deletion you are allowed.
- **Your own throwaway Postgres.** Template names include the migration digest. Keep separate per-track servers so concurrent migration/configuration work remains isolated. Start a container on your track's assigned port:
  ```bash
  docker run -d --rm --name deepref-agent-<track> -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=deepref \
    -p <port>:5432 pgvector/pgvector:0.8.7-pg17-bookworm
  export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:<port>/deepref TEST_DATABASE_URL=$DATABASE_URL
  ```
  - Run DB tests with `DEEPREF_REQUIRE_DATABASE=1` so they can't skip silently.
  - Stop your container (`docker stop deepref-agent-<track>`) when you finish.
  - Never point anything at the dev database (port 5432).
- **Disk.** Check `df -h /home` before each full build. Below 8 GB free, stop and report. Don't delete anything outside your worktree.
- **Shell.** The Bash tool runs fish. Wrap anything with `$(…)`, `&&` chains that need bash semantics, loops or heredocs in `bash -c '…'`.
- **Web.** In a fresh worktree, run `mise exec -- pnpm install --frozen-lockfile` first. Always run pnpm through `mise exec --`.

## Migration numbers (reserved, so parallel tracks don't collide)

| Track                  | Numbers   |
| ---------------------- | --------- |
| A jobs                 | 0059–0061 |
| B errors/observability | 0062–0063 |
| C AI integrity         | 0064–0066 |
| D assistant            | 0067–0069 |
| G smaller defects      | 0070–0072 |
| H architecture         | 0073–0074 |

Use only your own numbers. After adding or changing `query!` macros, run `cargo xtask sqlx prepare` (or `just sqlx-prepare`) against your container and commit the `.sqlx` diff. After an API change, run `just codegen` and commit the regenerated client.

## Files other tracks own

Stay inside your track's scope. Touch another track's hot files only for a mechanical, unavoidable edit (an import path, a match arm), and list it in your report.

| Track | Hot files                                                                                                                                             |
| ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| A     | `crates/app/src/jobs.rs`, `services/worker/src/lib.rs`, workflow node execution in `crates/app/src/workflows.rs` and `services/worker/src/workflows*` |
| B     | `crates/http-api/src/error.rs`, `routes/mod.rs` middleware, `routes/health.rs`, `crates/telemetry`                                                    |
| C     | `crates/app/src/ai_first/**`, `review_runs.rs`, `second_review.rs`, the review-run observer UI                                                        |
| D     | `crates/http-api/src/routes/assistant/**`, `crates/ai/src/runtime/**`, `services/worker/src/assistant*`, assistant web feature                        |
| E1    | `ScreeningFocus.svelte`, `FullTextScreening.svelte` keyboard handling                                                                                 |
| E2    | `crates/config`, `crates/http-api/src/config.rs`, `routes/actor.rs`, `apps/server/src/main.rs`, Dockerfiles                                           |
| F     | `.github/workflows/**`, root `Cargo.toml` `exclude`                                                                                                   |
| G     | documents block listing, `apps/web/orval.config.ts`, `ProjectWorkspace.svelte`, acquisitions depth, workflow secrets and hooks                        |
| H     | crate graph (`Cargo.toml` members), `crates/{core,events,graph,crossref,providers,grobid}`, `crates/application` ports, web dead code                 |

## Definition of done (per track)

- Each item has a test that you **saw go red** before the fix and green after, or a written reason why no test is possible.
- The handoff's verification gates pass at the end:
  - fmt;
  - `clippy --workspace --all-targets -D warnings`;
  - `nextest --lib --bins`;
  - your DB suites against your container;
  - `xtask boundaries`;
  - pnpm check/test/lint for web changes;
  - codegen-check for API changes.

  The baseline is in the handoff. Don't add lint warnings.

## Report back

Your final message is your report. Include:

- the branch name and commit list;
- for each item: `fixed`, `partial` (what is left), `no_change_needed` (why) or `blocked` (on what);
- the red-then-green evidence (test names);
- gate results with counts;
- every edit outside your scope;
- anything that contradicts the handoff.

## Owner decisions (2026-10-11, for wave 2)

- **Workflow engine:** park the niche nodes (Slack, HTTP, webhook, inbound e-mail) behind a flag and stop maintaining them. Keep the engine and the purpose-built automations, then unify the three run engines into one durable-run primitive.
- **Legacy ingestion plane:** retire it in all three phases this round (expand, migrate, drop the legacy tables).
- **Publishing:** after the orchestrator reviews each branch, it pushes the branch and opens a draft PR stacked on #209. Agents themselves still don't push.

### Wave 2 plan (starts from the integrated wave-1 branch)

| Wave-2 track | Work                                                                                                                                                                                  | Waits for   |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------- |
| H2           | typed seams: `JobKind` enum, typed state enums at the row boundary, all config through `deepref-config`; xtask forbids `sqlx` outside the persistence/app crate for migrated contexts | A, B, E2, H |
| H3           | `app::screening` pilot (Option B)                                                                                                                                                     | B, C, D, H  |
| H4           | durable-run unification, plus parking the niche workflow nodes                                                                                                                        | A, G, H     |
| H5           | legacy ingestion plane retirement, all three phases                                                                                                                                   | G, H        |

## Reviewed branch heads

| Track    | Branch                                                | Track head |
| -------- | ----------------------------------------------------- | ---------- |
| Packet 1 | `ci/run-db-integration-tests-with-isolated-databases` | `ee03cf4`  |
| E1       | `fix/screening-keyboard-guard`                        | `f21df1a`  |
| E2       | `fix/server-safe-defaults`                            | `8d92a4d`  |
| A        | `fix/jobs-hardening`                                  | `d068c9e`  |
| B        | `feat/api-errors-observability`                       | `0b81e76`  |
| C        | `fix/ai-first-and-review-retry`                       | `3027e82`  |
| D        | `fix/assistant-plan-governance`                       | `025163d`  |
| F        | `ci/coverage-and-scheduled-lanes`                     | `13069ed`  |
| G        | `fix/data-correctness-smalls`                         | `38ec8d1`  |
| H1       | `refactor/collapse-crates-and-dead-code`              | `7375bdd`  |
| H2       | `refactor/typed-runtime-seams`                        | `0244fba`  |
| H3       | `refactor/screening-context`                          | `c7b926f`  |
| H5       | `refactor/legacy-plane-retirement`                    | `f1d29bd`  |
| H4       | `refactor/durable-runs`                               | `63474a8`  |

Integration retains these commits and their authors, plus merge repairs. The final gates apply to `integration/review-resume`, not to each intermediate branch in isolation. Final backend/consumer repair commits are `cf872d3`, `06f6978` and `d122b33`; screening/SSE follow-ups are `32d6d7c`, `ef73348`, `5137664`, `47801c1` and formatting commit `c886db4`. Source branch histories remain available for individual commit review.

Review checkpoints `review/wave1-integrated` (`d402f57`) and `review/screening-integrated` (`ba0dffe`) preserve the dependency bases for wave 2 without rewriting track history. Their drafts are integration checkpoints, not additional feature work. Review individual track diffs first; merge through the checkpoints and then the final integration branch so generated contracts, migration repairs and consumer fixes are included.
