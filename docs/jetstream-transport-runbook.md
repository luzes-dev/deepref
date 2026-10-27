# JetStream delivery operations

PostgreSQL is authoritative. The JetStream adapter lives in
`deepref-integrations`; the existing worker loop still owns pool concurrency,
database leases, durable execution and graceful drain. Migrations 0156–0160
expand the existing generic outbox and add compatible job wakeup triggers.
The design decision is [ADR 0009](adr/0009-jetstream-work-delivery.md).

## Configuration

Select `WORK_TRANSPORT=jetstream` for the target deployment. `postgres` is the
explicit rollback/development setting and the compatibility default while
rollout is staged. `worker --pool documents` continues to serve only that pool;
without a pool selection the worker serves all existing classes. There is no
invented review class: durable review steps use workflow.

| Variable                  | Default                     | Contract                                                              |
| ------------------------- | --------------------------- | --------------------------------------------------------------------- |
| NATS_URL                  | required in JetStream mode  | nats:// in development; tls:// in production; no embedded credentials |
| NATS_TOKEN                | absent in development       | required secret in production; never put it in a URL                  |
| NATS_STREAM               | DEEPREF_WORK                | 1–64 ASCII letters, digits, underscores                               |
| NATS_SUBJECT_PREFIX       | deepref                     | bounded literal subject tokens; isolate environments/accounts         |
| NATS_REPLICAS             | 1 development, 3 production | 1/3/5; production requires at least 3                                 |
| NATS_MAX_BYTES            | 1073741824                  | 1 MiB–1 TiB, applies to each of the three streams independently       |
| NATS_MAX_AGE_SECS         | 604800                      | 60–2592000 seconds                                                    |
| NATS_ACK_WAIT_SECS        | 60                          | 3–3600 seconds; transport progress runs every ACK wait / 3            |
| NATS_REQUEST_TIMEOUT_SECS | 5                           | 1–30 seconds; ambiguous publish outcomes retry the same identity      |
| NATS_REPAIR_AFTER_SECS    | 900                         | at least five ACK waits, no more than stream age                      |

Streams capture `<prefix>.jobs.*`, `<prefix>.events`, a separate
`<prefix>.dead.>` DLQ, and metadata-only MaxDeliver advisories in
`<stream>_TERMINAL` on
`$JS.EVENT.ADVISORY.CONSUMER.MAX_DELIVERIES.<stream>.*`.
All three are file-backed, limits-retained, and reject new
messages when byte capacity is full. Envelopes are capped at 64 KiB. Stable
message IDs use `event_id:publication_epoch`; the 60-second broker dedupe window
is only an optimization. Configure NATS account quotas and token permissions
for these subjects and JetStream management before startup. Production TLS
uses the system trust store; private CA distribution is an operator concern.

Durable `work-<class>-v1` consumers share delivery across replicas of that pool.
They have explicit ACKs, five deliveries, 1024 pending ACKs, at most 256 waiting
pulls and a maximum pull batch of 256. The worker requests only available pool
capacity, up to its existing batch cap. `outbox-v1` consumes generic typed
contracts. Existing resource configurations must match the expected durable
policy; startup refuses drift instead of mutating live resources. Incoming subject transforms, republish/mirror/source routing, rollups, message-specific TTL/scheduling/counters and extra message/consumer limits are outside this policy and are refused. This prevents an acknowledged generic event from being redirected away from its filtered consumer.

## Cutover and rollback

1. Apply the expand migrations with the normal migration process. N-1 writers
   need no code changes: the database creates immediate wakeups and settles
   receipts on their lifecycle writes. Unknown JobKinds retain the existing
   decoder/capability fences; transport redelivery spends no business attempt.
2. Provision the scoped NATS account and storage; validate the settings in an
   isolated environment. Start a synthetic generic outbox pilot with a typed
   consumer and compare committed effects/receipts against PostgreSQL mode.
3. Drain PostgreSQL workers, then start selected workload pools with
   WORK_TRANSPORT=jetstream. Observe queue age, publish failures and pending
   consumer counts. Overlap is safe under DB fences but does not establish two
   permanent primary transports.
4. Switch remaining pools after equivalence/failure tests pass. The due
   publisher is normal scheduling; the 30-second repair scan is only repair.
5. To roll back, drain JetStream workers and restart with
   WORK_TRANSPORT=postgres. Keep migrations/triggers. The claim loop can serve
   the same queued jobs, and the DB settles wakeup receipts for either binary.
   Resume JetStream later using the retained queued/outbox state.

## Failure and replay

A publish ACK records `published_at` only. Publication errors are recorded in
`publication_error` and retry from `publication_next_at`; consumer attempts are
separate. Failed or ambiguous publication must never delete authoritative work.

Ordinary relay selection never republishes a row with `published_at` set.
`deepref_app::work_publication::replay_publication` is the explicit operator
persistence API for a pending published event. It preserves event identity and
committed consumer receipts and advances the broker dedupe epoch. For a
quarantined generic event, ordinary transport replay is unavailable. The
existing `replay_outbox_event` API is a separate business intervention after
operational review: it resets the event/quarantined-recipient attempt budget
and preserves delivered receipts. It must not be invoked merely to recover
missing transport diagnostics. Publication replay itself never resets receipts
or business attempts. These APIs require an approved maintenance context; no
public API exposes them.

When a current-generation notification arrives before available_at, the worker
ACKs only after persisting its due-publication obligation. The due publisher
emits/refreshes that generation when due. Stale generations terminate; completed
work ACKs; live claims ACK as no-ops; expired claims wait for existing lease
recovery. Recovered/retried work gets a new persisted generation and due time.
Consequential durable run/step checks and accepted attempt fences still apply.

`transport_dead_letters` is durable diagnostic evidence. Normal rejection
commits this record, publishes a bounded DLQ record, then terminates delivery.
DB failure prevents terminal ACK; DLQ failure retains any committed DB evidence.
There are at most five deliveries. **An unacknowledged fifth delivery does not
produce a sixth delivery.** No automatic DB/DLQ terminalization retry is promised
after exhaustion. Malformed data has no authoritative business replay identity;
its retained stream sequence is the recovery identity. Parser diagnostics use
closed category/location codes and never copy offending values or field names.

`<stream>_TERMINAL` retains MaxDeliver advisory metadata independently of the
worker and PostgreSQL. Its byte/age limits and account quota still apply: this
is bounded operational evidence, not an unlimited audit archive. Operators must
recover before retention expires and monitor evidence-stream capacity. If both
terminalization and advisory persistence fail or retention expires, inspect the
consumer's sequence/delivery counters and authoritative backlog; automatic
recovery of the missing diagnostic evidence is not guaranteed.

### Recover an exhausted sequence

1. Restore PostgreSQL/NATS availability and capacity. With explicitly scoped
   operator credentials and `WORK_TRANSPORT=jetstream`, run:

   ```bash
   deepref-server transport terminal-status --limit 25
   deepref-server transport terminal-status --after <next_after> --limit 25
   ```

   This read-only command never provisions streams or consumers. It reports
   retained first/last advisory sequence, cursor, count and whether this bounded
   page reached the observed end. An expired/ahead cursor, missing retained
   sequence or malformed advisory fails explicitly. Empty retention is not proof
   that no failures occurred. The whole operation has a 30-second deadline;
   limit is 1..100. No message body is printed.

2. Match the retained metadata to the incident and use its **advisory sequence**:

   ```bash
   deepref-server transport recover-terminal --advisory <advisory_sequence>
   ```

   The command loads that exact retained server advisory, requires this stream's
   finite-five contract, and resolves its `consumer` and original `stream_seq`.
   Foreign streams, malformed names, unexpected delivery budgets and missing
   advisory/work retention are refused. See the [NATS MaxDeliver advisory
   contract](https://beta-docs.nats.io/reference/2.14/jetstream/advisory/max-deliver).

3. Recovery idempotently commits `transport_dead_letters` evidence before the
   diagnostic DLQ copy, retaining an already committed consumer/reason/count.
   Rerun the same command after a partial failure. Its result explicitly states
   `business_replayed=false` and `delivery_budget_reset=false`. It does not create
   a sixth delivery, ACK an exhausted delivery, remove the shared message, run a
   handler, or change business attempts. This is diagnostic evidence under the
   database/operator credential, not a user-attributed financial adjustment.
   After timeout inspect persisted evidence before retrying; no all-or-nothing
   cross-system commit is promised. Retention/quota loss can still require manual
   incident investigation; this command cannot reconstruct absent evidence.
4. For a valid envelope with missing business receipts, inspect its authoritative
   outbox/job state and business budget. For a pending generic event, invoke
   `replay_publication(event_id)` and let the ordinary relay publish the new epoch.
   A quarantined/exhausted event stops here unless separate operational review
   explicitly authorizes the existing business `replay_outbox_event` flow, which
   resets only quarantined recipients' budgets and preserves delivered receipts.
   Do not perform that business intervention for diagnostic recovery. Transport
   publication replay never resets recipient budgets. Queued jobs use
   `repair_stranded_work`/their
   persisted generation; completed/cancelled work needs no invocation.
5. Verify the DB diagnostic row, DLQ metadata and authoritative receipt/state.
   Leave the original shared message to normal retention. Never reconstruct an
   ACK subject, delete a durable, replay malformed bytes indefinitely, reset
   attempts, or delete the shared original to clear an incident.

The procedure requires a maintenance context linked against these explicit APIs;
this slice adds no public/admin HTTP endpoint or credential-handling CLI. Real
service tests cover fifth-delivery DB and full-DLQ failures, absence of a sixth
delivery, retained advisory evidence and idempotent explicit diagnostic recovery.

Transport progress heartbeats prevent healthy slow handlers from exhausting
ACK wait. Database lease renewal remains independent and authoritative; a
worker losing its database fence cannot accept a consequential result because
it still has a transport heartbeat. Dropped/aborted deliveries stop progress
without ACK. Shutdown drains the existing worker tasks within the configured
telemetry shutdown deadline. Pool intake runs independently and each external
boundary has a 250 ms batch deadline and cooperative cancellation. Relevant DB
transactions also use local 250 ms statement/lock deadlines, leaving PostgreSQL
durability and unrelated pooled sessions unchanged. Stopping an intake returns earlier
accepted partial-batch claims for exact unstarted release and attempt refund.
A prepared claim fence is recorded before awaiting COMMIT; an ambiguous commit
attempts exact unstarted release, and an unconfirmed release is explicitly left
to its bounded authoritative DB lease. Handler execution never starts from an
unconfirmed commit. The repair/readiness branches also race shutdown.

## Visibility and alerts

Worker structured logs report pool pending, in-flight ACKs, redeliveries,
claim latency, pool saturation, event IDs/correlation and repair counts.
Monitor the authoritative backlog directly:

```sql
SELECT count(*), min(created_at), max(publication_attempts)
FROM outbox_events WHERE published_at IS NULL AND delivery_state='pending';
SELECT event_id,message_type,published_at,last_error
FROM outbox_events WHERE published_at IS NOT NULL AND delivery_state='pending'
ORDER BY created_at LIMIT 100;
SELECT workload_class,count(*),min(available_at) FROM jobs
WHERE state IN ('queued','capability_wait') GROUP BY workload_class;
SELECT consumer,count(*),max(recorded_at) FROM transport_dead_letters GROUP BY consumer;
```

`JetStream::with_observer(Arc<dyn TransportObserver>)` is the explicit parent
telemetry/health bridge. The adapter emits low-cardinality `Readiness`, per-pool
`Visibility` (pending, in-flight, redeliveries), `Claim` outcomes/latency, and
`ConsumeLatency` from outbox creation to intake. The publication supervisor
samples management readiness every 30 seconds with a bounded shutdown-aware
request; startup still validates provisioning and consumer policy.

The default observer is a no-op. This slice does **not** implement a metrics
exporter, an HTTP readiness bridge, time-series dashboards, or alert routes.
The parent telemetry implementation (frozen separately at a711) must install a
nonblocking observer, map these values into its instruments, and expose worker
NATS degradation independently of API availability. PostgreSQL-backed API
requests may remain available during NATS outage; worker JetStream readiness
must become degraded, never claim successful delivery from missing samples.

Parent operations acceptance must exercise failed/expired readiness, broker
outage/recovery, consumer/stream drift and metric sampling. Configure and test
alerts for oldest eligible queued work exceeding the pool SLO, sustained publish
errors, DLQ/terminal-evidence growth, repair growth and stream/quorum capacity.
These are required integration/operations obligations, not implemented alerts
in this source freeze. Event IDs/error strings/payloads must not be metric labels.

## Local development service

`docker compose --profile messaging up -d nats` starts the optional pinned,
file-backed local broker on loopback port 4222 (override NATS_HOST_PORT as
needed). Select WORK_TRANSPORT=jetstream and NATS_URL=nats://127.0.0.1:4222
in the local process environment. The profile retains a disposable nats-data
volume across restarts. Its one-node unauthenticated settings are development
settings; production config validation requires TLS, a token and replication.
The API/worker are still run through the repository's normal process workflow.

## Hermetic local/CI verification

Run `scripts/ci/test-jetstream.sh` on a Linux Docker runner with Rust and
cargo-nextest installed. It uses pinned NATS/pgvector image digests, random
loopback ports, labeled owned containers, a Cargo flock, memory/CPU caps and an
explicit 1 GiB PostgreSQL tmpfs for this bounded focused set. It sets matching
DATABASE_URL/TEST_DATABASE_URL and both strict dependency flags. Contract tests
use isolated database clones and subject prefixes and retain the configured
parallel PostgreSQL test group. The outage test runs afterward and pauses only
the name-and-label-checked fixture broker. EXIT cleanup removes both fixtures.
The script is CI-safe without secrets, production data or external providers.

This runner is not the full suite runner. Broad acceptance needs the parent’s
resource-guarded 14 GiB fixture and full diagnostic gates after independent
source review. No focused test result establishes full handoff/program acceptance.

## Required CI lane

`integration.nats` in `scripts/ci/diagnostics.toml` invokes the strict owned-fixture
runner, and `.github/workflows/ci-integration.yml` invokes that probe before the
integration gate. Both DATABASE_URL variables match the disposable PostgreSQL
instance; `DEEPREF_REQUIRE_DATABASE=1` and `DEEPREF_REQUIRE_NATS=1` fail closed.
PostgreSQL and JetStream have explicit bounded health waits. Pinned fixtures are
removed on exit and PostgreSQL volume/clone capacity is captured before cleanup.
The runner uses a 3 GiB PostgreSQL tmpfs with a 4 GiB memory ceiling and a 6 GiB
available-memory guard for this focused service set. It is not the broad-suite
runner; full database-suite capacity remains a separate parent gate.

An unconfirmed claim commit never starts a handler. Cleanup attempts the exact unstarted fence with a separate 250 ms budget, capped by the existing absolute shutdown deadline. This leaves positive statement time beyond the store's 50 ms commit margin. If the remaining drain time or database availability cannot confirm release, lease recovery remains the explicit fallback; refund is not guaranteed under every network ambiguity.
