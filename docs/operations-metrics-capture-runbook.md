# Local SDK metric capture

This drill checks the application's metric exporter and gauge capture contract
against an owned loopback receiver. It does not select a production metrics
backend or establish backend ingestion, alert delivery, fleet coverage or a
sustained SLO window. Use the [operations catalog](operations-observability-runbook.md)
for dashboard generation and the separate local alert-delivery drill.

## Prepare the source and dependencies

Use Linux, the repository's pinned Rust toolchain, Python 3.11 or newer and
`cargo-nextest`. The fixture uses `/proc` for exact process/lock identities. The
crate-local `operations-fixtures` feature enables optional dependencies; normal
production defaults and the default unit-test selection do not start a receiver.
No PostgreSQL database or migration is involved. The fresh-only
[database baseline](database-baseline-reset.md) is unaffected.

For a fresh dependency cache, coordinate a bounded dependency prerequisite under
the normal global Cargo lock before the capture slot. Resolve/fetch the checked-in
locked dependencies, including `opentelemetry-proto`'s `with-serde` dependencies:

```bash
cargo metadata --format-version 1 --locked \
  --filter-platform x86_64-unknown-linux-gnu \
  --features deepref-telemetry/operations-fixtures
```

Keep stdout JSON separate from Cargo's stderr diagnostics. Retain the original
lock identities and actual command exit. Release that prerequisite lock before
starting the capture runner, which owns its own nonblocking lock. The runner uses
offline Cargo and refuses missing dependencies rather than fetching them while
the fixture is running. It never activates features or edits source itself.

Create fresh source pins after all source/dependency changes:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/operations/test_otlp_capture.py \
  --pin-only --manifest /absolute/cache/new-sdk-source-pins.json
```

Pins cover every tracked repository input and the explicitly named fixture
files, including Rust/Cargo, SQLx metadata, embedded SQL/literal resources and
imported Python/catalog helpers. Do not reuse pins from another source epoch.
Stop source edits before compilation begins.

## Run the coordinated capture

Run only in an explicitly routed resource slot, without an outer flock or
inherited descriptor 9:

```bash
PYTHONDONTWRITEBYTECODE=1 timeout --signal=TERM --kill-after=10s 750s \
  python3 scripts/operations/test_otlp_capture.py \
  --manifest /absolute/cache/new-sdk-source-pins.json \
  --output /absolute/cache/new-sdk-capture-evidence
```

The new output directory must be absolute. At acquisition and before the fixture,
the runner requires at least 4 GiB free disk and 6 GiB available memory. It records
numeric initial/final values, uses the shared target with two build jobs,
debug-info/incremental settings from the shared build discipline, and refreshes
only its own Rust source mtimes before compilation.

Before any listener, it checks formatting, precompiles
`deepref-telemetry --lib --features operations-fixtures --locked`, runs scoped
all-target Clippy with warnings denied, and inventories the exact case
`otlp_capture::real_sdk_gauges_retain_source_time`. It then runs that one case with
zero retries and the existing nextest timeout policy. The runner's own budget is
720 seconds, individual compile commands are bounded to 180 seconds, and the
fixture command to 65 seconds. Setup/compile failures do not prove an ingestion
failure or semantic regression.

The Rust fixture requires the runner's exact inherited lock and owned output,
nonce and process identity. Ambient `OTEL_` and proxy settings are removed. Two
owned SDK providers use the production exporter policy factory and public
`WorkerMetrics`/`TransportMetrics` methods; global meter state is unchanged.
Only their two random fixture identities and fixed fixture service name are
accepted. The sole destination is its owned `127.0.0.1` ephemeral listener.

The receiver accepts at most four TCP streams, one concurrent RPC per connection,
two HTTP/2 streams per connection, sixteen requests and a sixteen-entry channel.
Decoded requests and retained artifacts are bounded to 256 KiB. The fixture uses
a 250 ms periodic-reader cadence and five-second phase waits. It does not use
`force_flush()` as a bounded primitive. The installed SDK's shutdown timeout
argument does not impose a hard end-to-end deadline; owned process containment
and final cleanup remain necessary.

## Interpret the evidence

Three phases must be received and assessed:

| Phase  | Source methods                                                                   | Required assessment                                                           |
| ------ | -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| Stale  | Queue age 7, successful sampler timestamp 91 seconds old, readiness true         | Queue unknown despite a fresh export timestamp                                |
| Fresh  | Update only sampler timestamp/status, leave queue gauge unchanged                | Queue 7 retained by the exporter policy; two fresh replicas produce 7, not 14 |
| Failed | Mark sampler failed, preserve last-success time and queue gauge; readiness false | Queue unknown; fresh transport readiness 0                                    |

Each received request is retained as decoded JSON and reencoded protobuf, with
typed roundtrip checks. These are not original compressed network frames. Final
accepted requests queued during shutdown are retained too. Each combined phase
must equal exactly two distinct retained received resource groups; its sidecar
names the source files, and the report hashes both artifact forms. Edited phase
data, replayed pointers, duplicate process identities or missing artifacts fail.

The converter joins database gauge values to `metrics.last_success` **values**
and `metrics.sample.up`, and transport readiness to its own `last_observed`
**value**. Export timestamps and heartbeats cannot rejuvenate source observations.
Failed, stale, future or incomplete evidence stays unknown; missing measurements
do not become zero. Only the fixed gauge scope/unit/dimension profile is mapped.
Counter, histogram and unbound query rows remain unknown.

On every result, inspect raw phase exits, `fixture/cleanup.json`, exact owned
PID/start identity, port, accepted/live connection and RPC/request counts,
temporary-file inventory, source hashes and final lock-release evidence. Providers
are shut down/dropped, then the receiver is stopped and joined. A one-second
graceful-join timeout triggers abort plus await and fails the drill. A successful
drill requires zero live connections/RPCs, all received requests retained, an
independently refused exact port connection, and no escaped owned process.
Emergency termination, temporary residue, source drift or cleanup errors fail
the final gate. Retain any missing outer process receipt as missing; a successful
child test is not a replacement for every runner/cleanup gate.

Only complete final success sets `local_fixture_sdk_gRPC_capture=true` in the
runner result. `backend_ingestion_verified` and `production_accepted` remain false.
The generic file converter always keeps its source-verification flag false; an
arbitrary file cannot authenticate its own origin.

## Convert a retained file

This command opens no listener and accepts no collector destination:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/operations/otlp_snapshot.py \
  --capture /absolute/cache/received-phase.json \
  --service-name operations-capture-fixture \
  --output /absolute/cache/new-normalized-capture
```

It writes `snapshot.json` and `provenance.json` to a new directory. Feed the former
to the existing catalog renderer. Preserve the source revision, actual capture
files and hashes with the conversion. An old retained capture correctly renders
unknown once its source timestamps expire.

Pure checks create modeled files only and do not establish SDK or wire proof:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s scripts/operations/tests -p 'test_otlp*.py' -v
```

The actual fixture requires its coordinated runner. Dedicated CI probe wiring and
full current-head gates are separate acceptance work. This capture also leaves
real negative runtime-watch → PostgreSQL heartbeat → HTTP recovery, three missing
source instruments, verified counter/histogram windows, deployed topology,
threshold/owner approvals and real on-call/backend acceptance open.
