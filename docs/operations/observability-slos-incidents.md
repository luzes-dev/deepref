# Observability, SLOs, and incident response

## Implemented observability assets

The repository contains four Grafana dashboards, six Prometheus rule groups, ADOT chart configuration, AMP/Managed Grafana/X-Ray/OpenTofu modules, and authenticated CloudWatch Synthetics source:

- `observability/dashboards/core-api.json`
- `observability/dashboards/data-plane.json`
- `observability/dashboards/projection-graph.json`
- `observability/dashboards/platform.json`
- `observability/alerts/*.yaml`
- `observability/synthetics/src/core-canary.ts`
- `observability/synthetics/src/access-denial-canary.ts`

The per-environment roots do not currently instantiate the observability module, and repository presence does not show that dashboards/rules/canaries were imported or that metrics exist. Treat all hosted telemetry and alert delivery as apply-time pending until a deployed evidence bundle proves collection, queries, alarms, and confirmed SNS delivery.

## SLI and SLO decision gate

The platform target is 99.9% monthly availability for core project and ingestion workflows. Neo4j graph/recommendation availability is excluded. The plan does not define a decision-complete numerator, denominator, latency threshold, exclusions, measurement source, low-traffic handling, burn windows, owner, or error-budget response.

Before production, the service owner must approve:

| Decision                   | Required record                                                                  |
| -------------------------- | -------------------------------------------------------------------------------- |
| Good-event numerator       | Exact routes/statuses and whether synthetics or server metrics are authoritative |
| Eligible-event denominator | Traffic classes, health routes, bots, maintenance, and zero-traffic behavior     |
| Latency objective          | Percentile, threshold, aggregation, and eligible routes                          |
| Exclusions                 | Clearly bounded planned maintenance and dependency/region exclusions             |
| Error budget               | Monthly calculation, multi-window burn alerts, freeze/escalation actions         |
| Ownership                  | Primary/backup responder and review cadence                                      |

The thresholds in `observability/alerts` are provisional symptom detection, not a complete 99.9% error-budget policy. Do not relabel them as SLO compliance without the decision above.

## What to observe

| Plane      | Primary signals                                                               | Source artifacts                                 |
| ---------- | ----------------------------------------------------------------------------- | ------------------------------------------------ |
| Core API   | request rate, 5xx ratio, latency, ready targets, dependency status            | `core-api.json`, `http-sli.yaml`, `/health/*`    |
| Data plane | JetStream stream depth/quorum, consumer pending, DLQ, retries, leases, outbox | `data-plane.json`, `nats-worker.yaml`            |
| Projection | projection lag/failures/rebuild, Neo4j health, entity revision, metric age    | `projection-graph.json`, `projection-neo4j.yaml` |
| Platform   | workload replicas/restarts, tunnel replicas/certificates, RDS, budgets        | `platform.json`, remaining alert files           |

Absence alerts are intentional. A missing metric is an observability failure, not evidence of health.

## Severity model to approve

The organization must approve its exact paging/communications policy. Until then use this conservative classification during drills:

- **SEV-1 candidate**: production core unavailable, suspected data loss/security compromise, or no safe recovery path.
- **SEV-2 candidate**: material production degradation, RDS/NATS redundancy loss, failed production migration, or rapidly growing error budget impact.
- **SEV-3 candidate**: graph-only degradation, bounded lag/capacity issue, or non-production incident without immediate production risk.

This classification does not name a paging vendor, owner, or response-time commitment. Those human decisions are production blockers.

## Incident lifecycle

1. **Acknowledge and classify**: validate environment and signal; open an incident record; name an incident commander and technical leads.
2. **Protect data and users**: stop promotions/migrations, preserve logs/queue/database evidence, and prefer degraded core service over speculative destructive repair.
3. **Diagnose read-only**: inspect dashboards, `/health/dependencies`, Argo, Kubernetes events, RDS, NATS, and projection status.
4. **Mitigate through ownership**: GitOps PR for workloads, protected OpenTofu apply for infrastructure, or the narrowly scoped approved runbook.
5. **Verify**: validate core/graph separately, queue/projection convergence, alerts, and user-facing behavior.
6. **Recover and communicate**: close only after the owner accepts stability and monitoring. Record residual risk.
7. **Learn**: complete a blameless post-incident review with timeline, contributing controls, evidence, and tracked actions.

Read-only triage:

```bash
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
argocd app get deepref-root --refresh
kubectl get pods --namespace "$NAMESPACE" -o wide
kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
aws rds describe-db-instances --region "$AWS_REGION" --db-instance-identifier "ambient-scribes-${ENVIRONMENT}"
nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" stream report
```

Never paste tokens/credential contents into incident chat or evidence. Use approved access and redaction.

## Communications and escalation

Before launch, define primary/backup on-call, paging target, incident channel, executive/user communication owner, security/data escalation, public-status policy, vendor escalation paths, postmortem threshold, and evidence retention. SNS is not a paging path until every intended recipient confirms the subscription and a test notification is acknowledged.

## Operational reviews

At an agreed cadence review SLI/error-budget results, alert noise/absence, incidents, capacity, costs, access, expiring certificates/credentials, backup restore currency, dependency/controller versions, and outstanding acceptance gaps. A review that finds apply-time evidence missing must keep the relevant acceptance criterion pending.
