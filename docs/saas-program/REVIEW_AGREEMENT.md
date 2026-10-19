# Review agreement: revisions to the SaaS handoffs

Two reviewers (Claude Opus 5.5 and GPT-6.1 Sol) reviewed handoffs 00–11 against the repository and agreed on the revisions below. The handoffs in this directory carry these revisions; this file records why.

## Repository facts the original handoffs missed

- `crates/app/migrations/0008_infrastructure_collapse.sql` dropped the former `event_outbox`, and `tools/xtask/src/boundaries.rs` bans `async-nats`. NATS was removed deliberately.
- `crates/app/migrations/0066_ai_autonomy_outbox.sql` is a domain-specific outbox drained by `crates/app/src/ai_autonomy_apply.rs`. It is not a generic publication outbox.
- The worker claims one job per 100 ms tick (`services/worker/src/lib.rs`, `run_claim_loop`). That is a loop design ceiling, not evidence that PostgreSQL is a bottleneck.
- AI budgets are per project (`projects.ai_monthly_budget_micros`, `crates/app/src/ai_usage.rs`) and are check-then-spend. Ledger write failures are logged and ignored (`crates/ai/src/usage.rs`, `crates/ai/src/runtime/hooks.rs`).
- `EnqueueJob` requires a `ProjectId` (`crates/application/src/jobs.rs`), and claiming excludes null projects (`crates/app/src/jobs.rs`). Workspace- or platform-scoped work cannot use the queue unchanged.
- `crates/app/migrations/0055_ai_first_blind_controls.sql` compares textual actor IDs against historical decisions and AI exposures to decide blinding and "fresh auditor" eligibility.
- `workflow_files.content` is `bytea` up to 16 MiB (`crates/app/migrations/0028_workflow_engine.sql`).
- An unset `APP_ENV` resolves to production today (`crates/config/src/environment.rs`). That fail-closed behavior must survive.
- `x-actor-id` is read in `crates/http-api/src/routes/actor.rs` and sent by `apps/web/src/lib/api/assistant-stream.ts`. HTTP already rejects client-supplied system/automation actor kinds; arbitrary human identity is still accepted.
- Assistant SSE (`crates/http-api/src/routes/assistant/run_events.rs`) replays persisted events by polling PostgreSQL, so it already works across API replicas.

## Agreed revisions (ranked)

1. **Enforceable hard AI limits (09).** Conservative reservation bounds; atomic top-ups before additional liability; settlement and recovery; pricing versions; period rollover. Replace warn-and-continue usage accounting with recoverable records. Keep project sub-limits beneath workspace and platform caps, and plan the migration from today's project budgets.
2. **Scientific identity continuity (03).** Explicitly handle legacy text actor IDs used by 0055's exposure and blinding checks. Canonicalizing identities must not make previously exposed reviewers appear fresh. Preserve historical provenance unchanged.
3. **One launch gate (00, 03, 07, 09).** Split 03 into migration, authorization and scientific-identity deliverables, released together. Run 07 alongside identity work. Paid AI requires complete reservation, settlement and recovery.
4. **Identity lifecycle security (01, 02, 03).** Keep fail-closed environment behavior. Remove browser actor-header authority. CSRF/Origin protection belongs in 02 with attack tests. Specify onboarding, WorkOS reconciliation, workspace-scoped membership revocation and global user deactivation.
5. **Proven tenant enforcement (03).** A route/resource coverage matrix covering reads, writes, streams, downloads and machine principals, each with a foreign-workspace test. Evaluate PostgreSQL row-level security as a separate decision.
6. **Broker-independent integration reliability (04).** A PostgreSQL integration inbox with authenticated receipt, deduplication, processing, reconciliation and ordering rules; a transactional outbox with safe claims. Without a broker, define the outbox's actual recipient and delivery criterion; claiming alone never marks delivery complete. Decide explicitly how `ai_autonomy_outbox` coexists rather than consolidating it automatically.
7. **Revised worker phasing (04, 05).** 05a now, on PostgreSQL: separate execution state from transport (`DurableExecutionStore` / `WorkTransport`), workload pools, claiming in bounded batches up to free pool capacity with responsive shutdown, and typed project/workspace/platform job scope. 05b: NATS stays the selected transport adapter, adopted when a written trigger fires. Triggers name thresholds, measurement windows and owners, and include demonstrated event fan-out/replay requirements alongside latency and database-load measurements.
8. **Execution/transport state transitions (05).** A precise ACK/NAK/due-time matrix covering stale generations, active claims, terminal states, not-yet-eligible work and delayed retries. Prevent multiplied retries. Keep fencing, reconciliation and graceful drain.
9. **Commercial readiness early (00, 08, 09).** Verify the actual seller's Polar payout eligibility before substantial billing work. Define privacy, retention, export/deletion (LGPD) and provider responsibilities. Keep immutable per-usage identities; verify Polar ingestion semantics before retries or aggregation.
10. **Audit starts with identity and tenancy (02, 03, 11).** Record authentication, membership and permission changes from the start. Bound denial-event volume and sensitive metadata. Keep administrative audit separate from scientific provenance.
11. **Explicit blob migration (07).** Include `workflow_files.content`, staged upload/backfill, integrity verification, authorized access and recoverable cleanup. Note that database-polled assistant SSE already spans API replicas.
12. **Valkey gated by limiter needs (06).** Implement shared admission controls when a distributed limiter is actually needed, with conservative outage behavior. Advisory locks alone do not implement rate limiting or leased concurrency.
13. **Selective CQRS (10).** Profile first; prefer same-transaction projections where suitable; require tenant isolation, rebuildability and explicit freshness before asynchronous projections.
14. **Operational acceptance (new 12).** Deployment ownership, expand/contract migration compatibility, backup/restore drills, SLOs and alerts, dependency-failure tests, CI services for any new infrastructure.

Program-level: revise 00's fixed decisions, dependency graph and completion criteria to match, and distinguish launch readiness from completion of the full target architecture.

## Points of disagreement that were resolved

- NATS is not replaced. It remains the selected transport, but cutover is trigger-gated. For durable jobs, the committed PostgreSQL row already retains pending work; NATS adds wakeup latency, fan-out and backpressure. Independent event consumers that need replay do require retained delivery, which is why fan-out/replay is itself a trigger.
- Polar usage export keeps one immutable identity per usage fact. Aggregation is allowed only after verifying Polar's ingestion, correction and billing-period semantics.
