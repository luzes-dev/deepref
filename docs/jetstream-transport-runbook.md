# JetStream delivery operations

PostgreSQL is authoritative. The JetStream adapter lives in
`deepref-integrations`; the existing worker loop still owns pool concurrency,
database leases, durable execution and graceful drain. Migrations 0156–0159
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
| NATS_MAX_BYTES            | 1073741824                  | 1 MiB–1 TiB, applies to main and DLQ streams independently            |
| NATS_MAX_AGE_SECS         | 604800                      | 60–2592000 seconds                                                    |
| NATS_ACK_WAIT_SECS        | 60                          | 3–3600 seconds; transport progress runs every ACK wait / 3            |
| NATS_REQUEST_TIMEOUT_SECS | 5                           | 1–30 seconds; ambiguous publish outcomes retry the same identity      |
| NATS_REPAIR_AFTER_SECS    | 900                         | at least five ACK waits, no more than stream age                      |

Streams capture `<prefix>.jobs.*`, `<prefix>.events` and a separate
`<prefix>.dead.>` DLQ. Both are file-backed, limits-retained, and reject new
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
policy; startup refuses drift instead of mutating live resources.

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
quarantined generic event, first use the existing `replay_outbox_event` API to
reopen its missing recipients, then request publication replay. These APIs
must be invoked through an approved operational maintenance context; no public
API exposes them. Do not reset committed receipts or business attempts.

When a current-generation notification arrives before available_at, the worker
ACKs only after persisting its due-publication obligation. The due publisher
emits/refreshes that generation when due. Stale generations terminate; completed
work ACKs; live claims ACK as no-ops; expired claims wait for existing lease
recovery. Recovered/retried work gets a new persisted generation and due time.
Consequential durable run/step checks and accepted attempt fences still apply.

`transport_dead_letters` is durable diagnostic evidence. A consumer first
commits this record, then publishes its bounded DLQ record, then terminates the
original delivery. DB failure prevents terminal ACK; DLQ failure leaves the DB
evidence and permits redelivery. Malformed wire data is identified by stream,
consumer and sequence; raw payloads are not copied into logs/DLQ. NATS
MaxDeliver after a process crash may leave the message in the retained stream;
queued jobs are repaired by the slow scan, while generic events with missing
receipts require explicit operational replay. The DLQ never authorizes business
failure or deletes the queued job.

Transport progress heartbeats prevent healthy slow handlers from exhausting
ACK wait. Database lease renewal remains independent and authoritative; a
worker losing its database fence cannot accept a consequential result because
it still has a transport heartbeat. Dropped/aborted deliveries stop progress
without ACK. Shutdown drains the existing worker tasks within the configured
telemetry shutdown deadline; abandoned claims recover through their DB leases.

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

Alert on oldest eligible queued work exceeding the pool SLO, sustained publish
errors, DLQ growth, repair growth, and stream capacity. Missing publication or
consumer visibility must never be interpreted as successful execution.

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
