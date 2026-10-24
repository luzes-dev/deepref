# Metrics pipeline and alert preparation

## Scope and evidence

The application exports OpenTelemetry metrics alongside traces when `OTEL_EXPORTER_OTLP_ENDPOINT` is configured. There is no new public metrics route. Without a collector endpoint, instruments are no-ops and the worker does not run the additional database sampler. The collector/backend is still an owner decision; this document does not claim a deployed pipeline, accepted SLOs or tested notifications.

Traces and metrics use the same service name, build version and random process instance resource identity. The instance identity distinguishes independently sampled replicas. Export failures do not block scientific or financial work. Exporter construction errors fail startup so an invalid configured endpoint cannot look enabled. Configure transport security and collector authentication for the eventual deployment; the application endpoint is a collector address, never a tenant-controlled URL.

The SDK's periodic metric reader exports on its configured interval (`OTEL_METRIC_EXPORT_INTERVAL` in milliseconds, subject to the SDK contract). The metric exporter has a five-second request timeout. Normal shutdown asks both providers to flush/shut down within the remaining configured shutdown budget; SDK/exporter shutdown is best effort and does not constitute a hard wall-clock or guaranteed delivery contract.

## Instrument catalog

These are OTLP instrument names. A backend may normalize punctuation or units; verify actual ingested names before installing alerts. Labels below are the complete application attribute sets. Resource attributes identify the service/process, never a customer.

| Instrument                                | Kind/unit           | Attributes                              | Meaning                                                                                         |
| ----------------------------------------- | ------------------- | --------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `deepref.http.requests`                   | counter, requests   | `route_class`, `method`, `status_class` | Responses started, including auth/host/origin rejections, unmatched routes and SSE handshakes   |
| `deepref.http.response_header.duration`   | histogram, seconds  | same                                    | Time to response headers, excluding responses with the SSE content type                         |
| `deepref.schema.behind`                   | gauge, boolean      | none                                    | Last successful readiness schema query: one if schema is older than this binary; zero otherwise |
| `deepref.worker.queue.depth`              | gauge, jobs         | `pool`                                  | Due queued and `capability_wait` jobs                                                           |
| `deepref.worker.queue.oldest_due.age`     | gauge, seconds      | `pool`                                  | Oldest due pending job; zero for an empty pool                                                  |
| `deepref.worker.in_flight`                | gauge, jobs         | `pool`                                  | Jobs in running state                                                                           |
| `deepref.worker.dead.depth`               | gauge, jobs         | `pool`                                  | All retained dead jobs; the jobs schema has no death timestamp                                  |
| `deepref.worker.capacity`                 | gauge, slots        | `pool`                                  | Sum of capacities advertised by heartbeats seen within 60 seconds                               |
| `deepref.worker.live`                     | gauge, processes    | none                                    | Heartbeats seen within 60 seconds                                                               |
| `deepref.backlog.depth`                   | gauge, rows         | `family`                                | Outstanding durable obligations, including quarantined rows                                     |
| `deepref.backlog.oldest.age`              | gauge, seconds      | `family`                                | Oldest corresponding obligation; zero when absent                                               |
| `deepref.usage_export.identity_conflicts` | gauge, facts        | none                                    | An immutable fact identity is occupied by a different outbox event                              |
| `deepref.usage_export.unattributed`       | gauge, facts        | none                                    | Retained unemitted facts without a resolvable workspace                                         |
| `deepref.worker.metrics.sample.up`        | gauge, boolean      | none                                    | One only after an entire database sample succeeds; zero on failure                              |
| `deepref.worker.metrics.last_success`     | gauge, Unix seconds | none                                    | Timestamp of the most recent complete database snapshot                                         |

`route_class` is a closed category: health, identity, webhook, billing, account, documents, import, assistant, research, other or unmatched. Methods collapse to the seven common methods or OTHER. Status values are 1xx–5xx or other. No URL, route parameter, query string or error is exported. Latency is response-header latency, not full download duration, full SSE duration or a measure of clients that disconnected before a response. External probes are still needed for end-user timeout/availability evidence.

`pool` comes from the canonical domain workload enum: ai, documents, workflow, integration or projection. `family` is outbox, autonomy_outbox, inbox, usage_settlement, usage_export or usage_unemitted. No workspace/project/user/customer identifier, provider payload, prompt, email, model name, cost or raw error is a metric attribute.

Usage settlement counts nonterminal reservations opened at least five minutes ago. It is diagnostic age, including legitimately running or provider-unknown liabilities, and never authorizes automatic voiding. Usage export includes emitted-but-undelivered events; usage_unemitted includes resolvable facts/corrections not yet represented by their matching export event. Unattributed and conflicting facts have their own gauges. Do not add overlapping backlog families to infer a unique fact total.

## Sampling and aggregation

Each configured worker samples every 30 seconds, with missed intervals skipped. It reads a repeatable-read, read-only transaction, a five-second outer deadline, 1.5-second statement timeout and 250-millisecond lock timeout. It publishes no partial sample. A query failure records sample.up=0 and does not replace last-known backlog counts with zero or advance last_success. An absent series is not evidence of an empty queue.

These gauges are global database aggregates sampled independently by replicas. Select samples with sample.up=1 and a recent last_success, then use **max across replicas**, not sum, for queue depth, backlog, live workers and already-summed capacity. A raw sum multiplies the actual population by sampler count. Use the process identity for freshness joins and alert if every sampler is absent/stale. Aggregate the HTTP counters/histograms across API replicas using the backend's normal counter/histogram semantics.

Schema.behind is updated by readiness probes; a failed database query does not fabricate a new schema result. Pair it with readiness probe failure/absence and HTTP health responses. A newly started service with no probe or sample must not look healthy merely because a gauge is absent.

## Symptoms, queries and actions

### Missing or stale metric samples

Symptom: no fresh complete sampler exists, or sample.up stays zero.

Query: inspect per-process sample.up and the age of last_success; compare current worker heartbeats and collector export health. Check database connection pressure and lock/statement timeouts. Metric queries intentionally give up under contention.

Action: restore the collector/database path or reduce sampling pressure in a reviewed change. Do not raise financial caps, clear obligations, restart expensive work or mark backlogs empty to repair a metric.

Evidence: retain the failed sample time, collector health and a later fresh complete sample. A restarted collector alone does not prove recovery.

### Queue age, dead jobs or missing capacity

Symptom: due age grows, dead.depth is nonzero, or expected live workers/capacity is missing.

Query: use the canonical workload pool and inspect `/health/dependencies` plus the [assistant durability runbook](assistant-durability-runbook.md). Distinguish delayed future work from due queued/capability_wait work.

Action: restore the affected pool or investigate terminal errors and fenced leases. Replays/requeues follow the existing operator runbooks; metric observations do not authorize deleting jobs or bypassing fences.

Evidence: show one terminal outcome per consequential job and a decreasing fresh due-age series.

### Integration or usage backlog

Symptom: outbox/inbox ages grow, usage facts are not emitted/delivered, or identity/unattributed gauges are nonzero.

Query: inspect the family-specific durable rows and the [provider outage runbook](provider-outage-runbook.md). Use the AI usage reconciliation/export operator diagnostics for immutable fact attribution.

Action: repair the dependency or mapping and replay with the same identities. Dodo ingestion rejects sufficiently old events; do not change timestamps/IDs to bypass its window. Keep unresolved facts visible and use an audited supported adjustment process.

Evidence: retained facts remain intact, receipts commit once, and fresh backlog/identity diagnostics show the actual outcome.

## Alert acceptance still required

Owners must accept every SLO target, window, page/ticket threshold, expected worker count and notification route before launch. Handoff 12 remains the acceptance matrix. This code does not assign an on-call person or invent production measurements.

Every alert needs its runbook section, owner and actual tested delivery evidence. At minimum cover response availability and route-class latency, readiness/schema mismatch, due age per pool, retained dead jobs, live workers, outbox/inbox and usage backlog, identity conflicts, stale samplers and collector failure. Add backup freshness/WAL lag, object-store error/latency and reconciliation orphan evidence when their source integrations land. Their absence here is remaining work, not a green signal.

Production acceptance also requires the chosen backend to preserve the bounded labels, an observed metric export, a delivered test notification for each route, and failure-injection evidence. No cloud provider or paid service has been selected or provisioned by this slice.
