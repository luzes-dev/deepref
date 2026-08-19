# Capacity, cost, and maintenance

## Capacity model

PostgreSQL is authoritative for state, durable jobs, graph reads, and metrics. API and worker are independently scalable stateless roles; the worker’s bounded concurrency and PostgreSQL lease queue provide the back-pressure boundary. Web, Cloudflare Tunnel, and telemetry remain separate from application data capacity.

## Baseline and scaling signals

| Component | Primary signals | First response |
| --- | --- | --- |
| PostgreSQL/RDS | CPU, connections, storage, lock waits, I/O latency, replica/failover state | inspect query/lock pressure, then resize or tune through OpenTofu |
| API | request rate, latency, 5xx ratio, ready replicas | scale API replicas and inspect database saturation |
| Worker | queued/running/dead jobs, oldest queued age, claim/retry rate, provider wait, processing duration | scale worker replicas within database/provider budgets |
| Web | gateway health, asset latency, error rate | inspect gateway and API path |
| Cloudflare/telemetry | tunnel replicas, probe health, collector queue | follow perimeter or telemetry incident procedure |

Scale only after checking the downstream database and provider budgets. More workers do not fix a saturated database or a provider throttle.

## Cost controls

- Keep local Compose PostgreSQL disposable and loopback-only.
- Keep environment sizes and replica counts in reviewed values/OpenTofu changes.
- Use ECR lifecycle policies, RDS retention controls, telemetry retention, and monthly budget alerts.
- Do not reduce deletion protection, backups, encryption, or admission policy to meet a budget.

## Maintenance windows

Freeze promotions during RDS maintenance, migration hooks, restores, major worker/provider incidents, and planned node/AZ drains. Use [node/AZ drain](runbooks/node-az-drain.md) and [migration failure](runbooks/migration-failure.md) for those changes.

## Evidence

Retain capacity graphs, queue age/attempt metrics, database saturation, cost/budget result, change approval, and post-maintenance health checks.
