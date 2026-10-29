# Local operations catalog, dashboard and delivery tests

## Artifact generation

The versioned [rule catalog](../operations/metrics/catalog.json) records source
instruments, units, bounded dimensions, replica reducers, capture policy, proposed
page/ticket thresholds, windows, owner roles, runbooks and test-delivery IDs.
Thresholds are unapproved local proposals. Hosting, secrets, the production metrics
backend and on-call destinations remain owner decisions.

Generate a local dashboard, proposed rule JSON and machine-readable assessments:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/operations/metrics_catalog.py \
  --catalog operations/metrics/catalog.json --repository . \
  --snapshot operations/metrics/snapshot.example.json \
  --output /tmp/deepref-operations-new-report
```

The output directory must be new. Open its `dashboard.html` locally. The empty
example snapshot produces unknown panels. `assessment.json` includes each panel's
capture expiry, value, proposed action and fresh/rejected replica counts.
`rules.proposed.json` retains the complete versioned catalog. The tool refuses
`--production`; these files do not activate a backend or notification route.

Use **Filter panels** to narrow the rendered rows by literal, case-insensitive
text, including panel dimensions, state, owner or runbook. Input is limited to 80
characters, and the visible/total count updates locally. Clear the input to show
all panels. Filtering makes no network request and never refreshes observations;
expiry still changes stale rows to unknown, including rows hidden by the filter.

Linux and Python 3.11 or newer supply the standard-library tooling; Node 24 or newer runs
the generated-script tests. No third-party Python package or hosted service is
required. Recorded evidence must include the actual interpreter versions.

## Capture and proposed rule contracts

A normalized snapshot has schema version 1 and an `observations` map keyed by the
expanded panel ID, such as `queue-age:pool=documents`. Each value is a list of
samples with exactly `instance`, `observed_at`, `complete`, `dimensions` and
`value`. Process identities are bounded opaque strings. A complete recent sample
of 70 seconds for that queue panel produces a ticket proposal against the
catalog's unapproved 60-second threshold. This compares one capture; it does not
establish a sustained five-minute alert window.

Global PostgreSQL aggregates use **max**, not sum, across complete fresh replicas.
Transport readiness uses min across fresh process observations; expected fleet
coverage is separate. The source adapter must use the database sampler's
`last_success` gauge value and `sample.up`, or the transport's own `last_observed`
gauge value. A recent metric export or worker heartbeat cannot refresh these
timestamps. A failed, empty, stale or future capture is unknown. Fresh zero remains
distinct from missing evidence. Rejected replica coverage remains visible even
when another complete fresh replica supplies a value.

The dashboard expires observations locally without making network requests.
Clock rollback cannot restore an expired observation; re-render with a new source
capture. The HTML and JSON assessments retain the render/capture boundary. JSON
assessments are point-in-time records, not continuously fresh monitoring.

The current normalized-file interface does not authenticate ingestion. Counter
ratios, histogram p95 and readiness-query rows stay unknown until a separately
verified query binding exists. A cumulative counter value is insufficient.
Backup/WAL, object-store errors and reconciliation retain explicit missing-source
rows. Their absence cannot become a healthy zero. No tenant, project, user, cost,
provider payload or raw error is a metric dimension.

For a missing/stale panel, first inspect its source gap, capture policy and
rejected/fresh counts. Verify the source observation and its process identity
before changing collectors or thresholds. For a proposed action, follow the
row's runbook and retain the exact catalog, normalized capture, generated
assessment and source revision. Unknown observations confer no claim, retry,
financial, deletion, drain or restoration authority.

## Bounded local wire delivery

The local delivery tool creates a receiver and sender in its own process, using
an ephemeral `127.0.0.1` listener. It accepts a catalog rule and an evidence path.
The receiver is bound to that rule, owner role, derived local route, canonical
catalog-content SHA-256, test ID, nonce and random token. It accepts a valid nonce
once. Wrong credentials or routing cannot consume the expected nonce.

Each acknowledgement binds the canonical complete request digest and original
identities. Frames are limited to 4096 bytes, exchanges to two seconds, clients to
four, the drill to twenty seconds and cleanup waits to one second. A stale,
future, missing or mismatched acknowledgement fails. The receiver closes its
accepted clients and tasks before waiting for server closure. An independent
connection attempt must confirm the exact owned listener has closed.

Run inside an explicitly coordinated fixture batch that already holds the normal
global lock on descriptor 9 and has passed the current disk/memory guards:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/operations/alert_delivery.py \
  --catalog operations/metrics/catalog.json --repository . --rule queue-age \
  --output /tmp/deepref-alert-new-evidence.jsonl
```

The CLI refuses a missing or foreign lock. It writes the exact owned port before
exchange, then a final JSONL receipt or sanitized failure type. Output must be a
new file. Keep the outer runner's initial/final guards, source hashes, raw exit,
independent exact-port closure and lock-release evidence too. Cleanup failures
fail the batch. Tokens and raw frames must not enter reports.

A successful fixture receipt has `wire_delivery_verified=true` and
`local_fixture_delivery=true`; `production_accepted`, `human_notification_verified`
and `backend_ingestion_verified` remain false. A pure parser receipt keeps
`wire_delivery_verified=false`. Actual production notification acceptance requires
the selected backend and a retained delivered test to the selected operational
destination, with owner-approved targets and routing.

## Checks and remaining acceptance

Pure source checks open no listener:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s scripts/operations/tests -p test_metrics_catalog.py -v
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s scripts/operations/tests -p test_alert_protocol.py -v
DEEPREF_DASHBOARD_HTML=/path/to/generated/dashboard.html \
  NODE_DISABLE_COMPILE_CACHE=1 node --test scripts/operations/tests/test_dashboard_expiry.cjs
```

`test_alert_delivery_wire.py` is the separate real socket selection. It requires
the inherited fixture lock and an exact runner-owned
`DEEPREF_ALERT_PORT_MANIFEST`; missing prerequisites fail. It checks matching ACKs,
binding/time drift, real missing-ACK timeout, replay refusal, invalid routing
followed by valid delivery, partial-client shutdown and owned listener/task
closure. It must run in a routed slot; pure passes are insufficient wire evidence.

The checked-in runner owns its nonblocking flock and accepts previously saved
source pins. Create the pins off-lock after source preparation, then execute only
when the coordinator routes this short slot:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/operations/test_observability.py \
  --pin-only --manifest /absolute/cache/new-source-pins.json
# At the explicitly routed slot, without an outer flock:
PYTHONDONTWRITEBYTECODE=1 timeout --signal=TERM --kill-after=2s 90s \
  python3 scripts/operations/test_observability.py \
  --manifest /absolute/cache/new-source-pins.json \
  --output /absolute/cache/new-observability-evidence
```

The runner checks at least 4 GiB disk free and 6 GiB available memory after
acquisition, captures initial/final numeric values and source identities, and runs
19 catalog, 10 pure protocol, nine wire and seven generated-script tests plus the
actual catalog-bound CLI drill and documentation/format checks. Its wire manifest
records 14 listener lifetimes and their task/refusal/deadline inventory; the CLI
adds one listener. Every exact owned port is independently rechecked. Temporary
fixture directories must be empty. Formatter temporary/cache directories are
separate; derived compilation files are retained and manifested. Failed or
incomplete phases, cleanup errors and source drift fail the final result.

These counts describe the required runner selection. An actual `result.json` and
its raw logs establish which tests ran; authored source is insufficient evidence.

Local wire checks still do not establish actual exporter ingestion, verified
counter/histogram queries, sustained SLO windows, missing source instruments,
production threshold signoff, on-call delivery or deployed NATS TLS/quorum and
Valkey topology. Keep those gates open in the operations acceptance matrix and
the [metrics pipeline runbook](metrics-and-alerts-runbook.md).
