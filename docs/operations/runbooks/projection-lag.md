# Runbook: projection lag

## Purpose and scope

Diagnose and recover delayed PostgreSQL-to-Neo4j projection while keeping PostgreSQL-backed core workflows available. Prefer restoring normal consumer progress; rebuild only when cursor/parity integrity cannot be recovered incrementally.

## Safety warnings

- Do not clear Neo4j, reset the durable consumer, alter projection cursors, republish domain events, or edit `projection_state` during initial triage.
- Do not scale/patch the projector directly. Resource or replica changes go through reviewed chart/GitOps release source.
- Lag measured from absent telemetry is unknown, not zero.
- A healthy `/health/ready` is expected during graph degradation and does not mean graph recovery is complete.

## Prerequisites and authorization

- Incident/change record, service/projector owner, platform operator for hosted inspection, and data owner if cursor/rebuild decisions arise.
- Read access to API, Argo, Kubernetes, dashboards, NATS observer credentials, and approved database diagnostics.
- Current release lock, known migration/rebuild activity, and no concurrent graph maintenance.

## Triggers and symptoms

- `DeepRefProjectionLagHigh`, increasing `deepref_projection_lag_seconds`, or projector consumer pending grows.
- `/health/dependencies` shows projection degraded/unavailable.
- `/projects/{project_id}/projection` shows nonzero/rising lag, old `last_success_at`, failure, or rebuilding.
- Graph responses are stale or return `GRAPH_UNAVAILABLE` while core remains usable.

## Ordered steps

1. Confirm scope and preserve core service:

   ```bash
   curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
   curl --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
   curl --silent --show-error \
     "https://REPLACE_WITH_ACCESS_HOST/api/projects/REPLACE_WITH_PROJECT_UUID/projection"
   ```

2. Inspect Argo/projector/Neo4j state and recent events:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get pods,deployments,statefulsets --namespace "$NAMESPACE" -o wide
   kubectl logs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=projector --tail=500
   kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
   ```

3. Inspect the durable consumer without mutation:

   ```bash
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" \
     stream info DEEPREF_DOMAIN --json
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" \
     consumer info DEEPREF_DOMAIN deepref-projector --json
   ```

4. Classify the bottleneck:

   - NATS quorum/consumer inactive: use [NATS quorum and DLQ recovery](nats-quorum-dlq-recovery.md).
   - Neo4j unavailable/storage/heap/query failure: stabilize node/PVC/credentials through GitOps/platform ownership.
   - Projector crashes or poison domain event: identify event/revision, preserve DLQ/failure evidence, fix application source.
   - Sustained valid load: review CPU/memory/batch/replica capacity and propose a staged chart change.
   - Rebuild active: monitor its run ID/stages; do not run normal remediation concurrently.
   - Cursor/parity suspect: stop incremental mutation and prepare [Neo4j rebuild](neo4j-rebuild.md).

5. Apply the smallest owning-source fix. For application/chart changes, build/promote a new immutable release. For node/storage infrastructure, use reviewed OpenTofu/GitOps and the relevant maintenance runbook.

6. Observe consumer pending, highest applied revision, failure rate, and lag until they monotonically converge. Avoid declaring recovery from a single sample.

## Verification

Verify projector pods ready, Neo4j available, `deepref-projector` pending/ack-pending decreases to the accepted baseline, projection state becomes `ready`, revision advances, lag is zero/within the approved objective, graph/recommendation responses recover, metric freshness updates, and core remains healthy.

## Rollback or safe stop

Close unmerged remediation PRs when assumptions fail. Roll back a same-migration application/chart change through the protected workflow. If lag grows, failures recur, or cursor integrity is unclear, stop incremental actions and escalate to rebuild/data review. Never reset the consumer or database cursor to make a metric look current.

## Escalation

Escalate NATS/cluster issues to platform, projector defects to service owners, Neo4j issues to graph/platform support, cursor/parity ambiguity to the data owner, and sustained production graph impact/target breach to the incident commander.

## Evidence and audit capture

Retain alert/query timestamps, dependency/project status, NATS consumer report, pod/events/log excerpts, bottleneck classification, source/GitOps change and approvals, lag/revision convergence graph, graph/core checks, and rebuild or follow-up decision.
