# Observability, SLOs, and incidents

## Supported signals

The active telemetry contract covers API request rate/latency/5xx, dependency availability, PostgreSQL health, worker queue depth and oldest age, job claims/retries/dead counts, provider wait, processing duration, web health, Cloudflare Tunnel health, and collector delivery. Graph freshness is a PostgreSQL metric/status value, not a separate service dependency.

## SLO ownership

- API owner: availability, latency, error ratio, readiness.
- Worker/data owner: queued age, retry/dead rate, lease recovery, ingestion completion, metric freshness.
- Platform owner: RDS, Kubernetes, Cloudflare Tunnel, telemetry, and alert delivery.

Thresholds and notification routes must be reviewed per environment. An alert without a tested notification route is not operational evidence.

## Incident first response

1. Declare the incident and freeze promotions, migrations, restores, and maintenance that could obscure the signal.
2. Check `/api/health/live`, `/api/health/ready`, `/api/health/dependencies`, API/worker replicas, PostgreSQL events, and queue age.
3. Separate API availability from database saturation, provider throttling, lease recovery, Cloudflare, or telemetry failure.
4. Preserve logs/metrics and use the relevant runbook. Avoid direct workload mutation except for approved break-glass action.
5. Verify recovery with health endpoints, durable job convergence, graph metric freshness, and user-facing synthetic checks.

## Evidence

Capture incident timeline, alert payload, dashboard links, health responses, database/job diagnostics, deployment/GitOps identity, mitigation approval, and follow-up owner/date.
