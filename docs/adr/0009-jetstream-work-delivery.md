# ADR 0009: JetStream delivery with PostgreSQL execution authority

Date: 2026-10-23. Status: implementation decision; rollout requires operational validation.

The product owner explicitly brought handoffs 04b and 05b into current scope on
2026-10-23, replacing the earlier T-NATS measurement/adoption gate. This is the
decision evidence; no measured threshold is claimed. It reverses the deliberate
broker removal in migration 0008 and narrows the former blanket async-nats ban
to permit the integrations adapter layer. Domain, application and persistence
remain independent of that SDK.

JetStream delivers identifiers. PostgreSQL retains jobs, runs, steps, attempts,
business retry times, accepted attempt identity and consequential fences. This
is still one modular application, with the existing worker binary and pool
controls. A message can never authorize a side effect.

The generic outbox retains its existing rows and frozen recipient contracts.
Publication has a separate fenced lease, attempt count and error/backoff; a
broker persistence ACK sets published_at, never delivered_at. Ordinary retries
exclude already-published rows. Deliberate replay increments a publication
epoch, keeping event identity and completed consumer receipts. JetStream's
message-ID deduplication is an optimization, not a correctness boundary.

Successful job writes create immediate wakeups in the same transaction, including N-1
writers. A bounded due publisher handles future available_at generations;
concurrent publishers emit one event per generation. Lifecycle settlement
records the durable-work receipt in that same database transaction. The worker
ACKs after settlement; a crash between commit and ACK replays as a no-op. A
slow repair scan republishes stranded queued generations after a configured
age. It never claims jobs or increments business attempts.

Limits retention supports independent durable consumers and explicit replay.
Each workload class has one shared durable pull consumer. File storage,
explicit ACKs, five deliveries, bounded pending messages, storage/age limits
and deterministic configuration checks are mandatory. Stream capacity rejects
new publications rather than evicting recent work to make room. A bounded DLQ
stream is diagnostic; PostgreSQL records dead-letter evidence before DLQ
publication and terminal acknowledgement. Exhaustion does not dead-letter the
business job; queued work remains repairable. At the fifth delivery a failed
DB/DLQ write has no automatic sixth-delivery retry owner. A separate bounded
file stream retains metadata-only MaxDeliver advisories; explicit retained-
sequence maintenance recovers diagnostic evidence. The runbook defines the
manual recovery procedure and evidence-retention limits.

Transport progress heartbeats run independently of database lease renewal.
Neither renews the other. Dropping a delivery stops progress without ACKing;
losing a DB fence still stops or rejects consequential work, even if broker
progress continued. Shutdown stops intake and drains existing handlers within
the existing deadline. Independent pool intake returns earlier accepted claims
from a cancelled partial batch for exact unstarted release. A prepared fence is
registered before awaiting claim COMMIT so an ambiguous response remains tied
to an exact release attempt or bounded authoritative lease recovery.

WORK_TRANSPORT=jetstream is the target deployment setting. postgres remains a
controlled development/rollback mode during rollout; no process runs both
primary queue implementations. The API keeps accepting authoritative
transactions during a broker outage and records an outbox backlog. New worker
startup fails closed if provisioning/configuration validation fails. Existing
workers retry publication, and resume delivery after recovery.

See [the transport runbook](../jetstream-transport-runbook.md) for configuration,
cutover, replay, repair, visibility and local/CI verification. No external
deployment or full program acceptance is implied by this ADR.

Review correction, 2026-10-25: migration 0160 separates BEFORE generation
calculation from successful AFTER publication, including dedupe conflicts.
Targeted generic claiming settles expired recipient reservations and enforces
the existing per-recipient budget/backoff before invocation. Structural wire
errors discard Serde source text. Consumer policy validation includes body,
replay and durability settings. Required CI runs the strict `integration.nats`
probe. Telemetry observer/readiness hooks are integration seams; parent metrics,
HTTP readiness exposure and tested alert routing remain separate acceptance.
