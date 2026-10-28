# Bounded local fault injection

`scripts/operations/fault_proxy.py` forwards opaque TCP bytes to an explicitly
owned disposable Docker fixture. Use it for an outage/recovery test, including
an intentionally ambiguous response. It does not establish a rollback, a
successful business effect, managed failover, PITR or production acceptance.

Symptom under test: a dependency stops answering or disconnects while durable
work is outstanding. Choose the relevant authoritative receipt/fence invariant
before injecting a fault; a recovered TCP connection alone is insufficient.

## Query and ownership checks

The upstream must have a full container ID, an exact `deepref-ops-…` name and
unique `deepref.task=operations-…` label, and exactly one published binding for
the selected container port, on `127.0.0.1`. The tool checks active/unpaused state
and identity twice before listening. It never creates/removes services, reads an
application `DATABASE_URL`, chooses a destination from control input, or logs
application bytes or Docker environment/secrets. Local labels select fixtures;
they do not protect against a malicious host administrator replacing a listener.

Use Python 3.11 or newer on Linux/POSIX with Docker for the CLI. The Python
socket tests need no Docker service.

The harness must hold the global fixture/Cargo flock and inherit descriptor 9:

```bash
exec 9>"$HOME/.cache/deepref-impl/cargo-build.lock"
flock -n -x 9 || exit 3
# Create and verify the exact owned fixture only after resource guards pass.
# Keep default PostgreSQL durability, configured test parallelism and timeouts.
python3 scripts/operations/fault_proxy.py \
  --container-id "$OWNED_CONTAINER_ID" \
  --container-name "$OWNED_CONTAINER_NAME" \
  --task-label "$OWNED_TASK_LABEL" \
  --container-port 5432 --lifetime-seconds 120
# Capture exact service settings/df/clone inventory and remove only the verified
# owned IDs/anonymous volumes; independently verify absence before releasing 9.
```

The tool refuses an unheld/wrong inode or another process's independent lock.
It inherits the lock; it does not release the harness's lock or nest a runner
that takes another flock. A failed guard or busy flock means no fixture and no
queued retry. Parent/orchestrator coordination still applies.

## Action: inject and recover

One JSON line on stdout reports random loopback `proxy_port`, `control_port`, a
per-process `control_token`, and the maximum process lifetime. Keep that local
fixture token out of shared logs. Send one newline-terminated JSON object to the
control port:

```json
{ "token": "<from this process>", "mode": "stall_response", "seconds": 5 }
```

Modes are `pass`, `drop`, and `stall_response`. Each command lasts 1–30 seconds
then restores `pass`; changing mode replaces the prior expiry. `drop` aborts
existing data paths and refuses new ones. `stall_response` forwards requests to
the upstream while withholding responses. A timed-out client may therefore have
committed a write. Recovery tests must check authoritative durable identities,
leases, fences and receipts, and retry by stable identity rather than resetting a
business retry counter. The proxy never interprets SQL or acknowledges a write.

Limits are 16 data connections, four control connections, 64 KiB read chunks,
64 KiB transport write high-water marks, 30 seconds per data connection, 4 KiB
control lines, two seconds per control request plus at most one second to close its transport,
and at most 300 seconds per proxy
process. Stream/socket/kernel buffers are additional to the read chunk; this is
not a claim of exactly 64 KiB total memory. Excess admission is closed. Half-close
forwards EOF and allows the bounded opposite direction to finish. SIGINT,
SIGTERM and process expiry close both listeners and all owned traffic/tasks.
Aborted sockets may report EOF or connection reset; neither authorizes assuming
an upstream write rolled back.
Control accepts exactly token/mode/seconds and never reflects invalid input.

## Evidence and limits

Run the socket/catalog/lock tests in a routed owned-fixture slot:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s scripts/operations/tests -p 'test_fault_proxy.py' -v
```

These tests use actual process-owned loopback echo listeners and actual file
locks. Catalog parser cases are not evidence of real Docker provisioning. A
separate real-fixture CLI test is required before adopting the CLI in service
runners. No CI probe is activated by this tooling milestone.

The complete failure matrix remains a separate acceptance requirement: default-
durable PostgreSQL ambiguous commit plus actual stale-worker consequential fence
and inbox/outbox recovery; TLS object-store upload/metadata/cleanup; recorded
WorkOS and Dodo provider outages with immutable event/financial facts; actual
NATS stable event/recipient recovery and finite-five terminal handling; and
Valkey loss with conservative PostgreSQL fallback. Local proxy evidence does not
replace external provider approval, managed topology, production alert delivery,
backup retention or signed RPO/RTO.
