# Runbook: Neo4j rebuild

## Purpose and scope

Recreate the single-node Neo4j graph read model from authoritative PostgreSQL, replay domain events newer than the snapshot watermark, verify count/hash parity, and resume projection. Core project/ingestion operations should remain available while graph routes return typed degradation.

## Safety warnings

- Rebuild clears Neo4j. It must never alter PostgreSQL authority, and it is not a remedy for suspected PostgreSQL corruption.
- Do not run `deepref-projector rebuild` interactively in a hosted projector pod or create an ad-hoc Kubernetes Job.
- The Helm source renders a rebuild Job only with `rebuild.enabled=true` and a unique UUID `rebuild.runId`. Current GitOps policy permits deployment PRs to change only release locks; there is no repository workflow that safely opens/approves a one-off environment values change. Hosted rebuild is therefore blocked until an App-authored values-change workflow and matching policy are implemented and applied.
- Never disable graph degradation/readiness controls or make Neo4j authoritative to avoid a rebuild.

## Prerequisites and authorization

- Incident/maintenance record; data owner, service owner, and platform operator approval; production requires protected GitOps approvals.
- PostgreSQL healthy/current, domain-event/outbox state understood, enough Neo4j storage, no migration/PITR/promotion, and an accepted backup status.
- Baseline work/membership/citation counts and sampled deterministic hash method.
- The missing App-only rebuild workflow/policy has been implemented, reviewed, tested in development/staging, and retains the environment values audit trail.
- Representative performance test and at least one deployed staging rebuild completed successfully before production.

## Triggers and symptoms

- Neo4j unavailable/corrupt or constraints cannot be trusted.
- Projection cursor/parity cannot be safely repaired incrementally.
- Approved acceptance/DR rebuild drill.
- Do not rebuild for ordinary bounded projection lag until [projection lag](projection-lag.md) triage completes.

## Ordered steps

1. Confirm core health and graph-only degradation:

   ```bash
   curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
   curl --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
   kubectl get pods,statefulsets,pvc --namespace "$NAMESPACE" -o wide
   argocd app get deepref-root --refresh
   ```

2. Stop if PostgreSQL is unhealthy, a restore/migration/promotion is active, or graph failure affects core authority.

3. Capture projection status and baseline:

   ```bash
   curl --silent --show-error \
     "https://REPLACE_WITH_ACCESS_HOST/api/projects/REPLACE_WITH_PROJECT_UUID/projection"
   kubectl logs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=projector --tail=300
   ```

4. Generate a unique run ID:

   ```bash
   export REBUILD_RUN_ID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
   printf '%s\n' "$REBUILD_RUN_ID"
   ```

5. Through the approved deployment-App workflow (which must exist before this step), propose an environment values change setting only:

   ```yaml
   rebuild:
     enabled: true
     runId: REPLACE_WITH_REBUILD_RUN_ID
   ```

   Review the rendered `charts/deepref/templates/projector-rebuild-job.yaml`, immutable projector digest, target environment, active deadline, batch size, and approvals. Do not perform this change by human direct push.

6. Merge the approved GitOps PR and observe the named Job/Argo sync:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get jobs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=projector-rebuild -o wide
   kubectl logs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=projector-rebuild --follow
   ```

   The command performs advisory lock, repeatable-read snapshot/watermark, projection pause, clear/migrations, bounded load, replay, verification, and ready/resume. One advisory lock prevents concurrent rebuilds.

7. When the Job succeeds, use a second App-authored GitOps PR to restore `rebuild.enabled=false` while retaining the run/evidence record outside runtime values.

## Verification

Verify Job `Complete`, projection state `ready`, lag converges to zero, last success/revision advances, Neo4j ping/graph routes recover, core stayed ready, counts and sampled hashes match, and no projection failure alert remains. Acceptance requires the 250k-work/2.5m-edge staging/production-like rebuild to complete under sixty minutes with retained parity evidence.

## Rollback or safe stop

Before merge, close the rebuild PR. After start, do not launch another run ID or kill/delete the Job unless the incident commander/data owner declares a safe stop. A failed rebuild leaves graph degraded; PostgreSQL remains authoritative. Preserve Neo4j/PVC and logs for diagnosis, fix source/configuration, and start a new uniquely identified approved run. Never restore an unverified Neo4j snapshot over authority.

## Escalation

Escalate PostgreSQL/snapshot ambiguity to the data owner, Neo4j storage/query failures to platform/vendor support, parity/replay defects to projector maintainers, and a target breach or core impact to the incident commander.

## Evidence and audit capture

Retain App workflow/PR/approvals, release/projector digest, run ID, baseline/watermark, eight-stage logs with redaction, start/end/duration, counts/hashes, projection/graph/core health, alerts, safe-stop decisions, and cleanup PR. Record the current missing workflow/policy as a blocker until resolved.
