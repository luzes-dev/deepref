# Runbook: NATS quorum and DLQ recovery

## Purpose and scope

Recover JetStream quorum/consumer progress and triage dead-lettered events without losing PostgreSQL authority or bypassing GitOps. Hosted streams are `DEEPREF_WORK`, `DEEPREF_DOMAIN`, and `DEEPREF_DLQ`; durable consumers are `deepref-worker` and `deepref-projector`.

## Safety warnings

- Never delete/recreate a stream, consumer, StatefulSet, PVC, or NATS account as an initial recovery step.
- Never scale hosted NATS to an even replica count or drain enough nodes/AZs to lose the two-of-three quorum.
- Application credentials are subject-restricted. Use the approved observer credential for inspection and the admin credential only under platform authorization.
- Do not blindly republish raw DLQ payloads. The repository has no supported bulk DLQ replay command; malformed or obsolete events may poison consumers again.
- PostgreSQL claims/outbox/domain events are authoritative for recovery decisions.

## Prerequisites and authorization

- Incident/change record, platform operator, application/data owner for DLQ decisions, and approved short-lived NATS observer credential file.
- Private network path and `NATS_URL`, `NATS_CA_FILE`, `NATS_OBSERVER_CREDS` supplied without logging contents.
- Current GitOps lock/chart values and awareness of active node/AZ maintenance.
- Admin credential and GitOps approval only if configuration correction is required.

## Triggers and symptoms

- `DeepRefNatsQuorumAtRisk`, missing stream leader/replicas, unavailable NATS pods, or publish/consume errors.
- `deepref-worker`/`deepref-projector` pending or ack-pending count grows.
- `DeepRefDlqDepth`, records in `dead_letter_records`, malformed events, or exhausted five-delivery policy.

## Ordered steps

1. Freeze promotions/drains and establish current state read-only:

   ```bash
   kubectl get pods,pvc,statefulsets --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=nats -o wide
   kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" server report jetstream
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" stream report
   ```

2. Inspect every stream and durable consumer:

   ```bash
   for stream in DEEPREF_WORK DEEPREF_DOMAIN DEEPREF_DLQ; do
     nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" stream info "$stream" --json
   done
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" \
     consumer info DEEPREF_WORK deepref-worker --json
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" \
     consumer info DEEPREF_DOMAIN deepref-projector --json
   ```

3. Identify whether the cause is pod/node/AZ loss, storage/PVC, TLS/account credential, bad GitOps configuration, or application backpressure. Check pod logs with secret redaction:

   ```bash
   kubectl logs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=nats --all-containers --tail=300
   kubectl get nodes -L topology.kubernetes.io/zone
   ```

4. Restore the missing node/AZ through its owner. For a node issue, follow [node/AZ drain](node-az-drain.md) in reverse and let the StatefulSet/PVC reschedule. For a chart/config/credential issue, open a reviewed GitOps values/source fix; do not patch the StatefulSet or ConfigMap.

5. Wait for all three hosted stream replicas and a leader before resuming maintenance. Confirm consumer pending counts are stable/decreasing and application outbox publishing recovers.

6. For DLQ, capture metadata and classify each reason. View a bounded sample only; payloads may contain sensitive bibliographic/request data:

   ```bash
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" \
     stream info DEEPREF_DLQ --json
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" \
     stream view DEEPREF_DLQ --count 20
   ```

7. Correlate `event_id`/payload hash with `dead_letter_records`, `processed_events`, `event_outbox`, and ingestion state using the approved private diagnostic database role. Choose one action per class:

   - malformed/unsupported: retain evidence and fix the producer/compatibility; do not replay;
   - transient provider failure after exhaustion: correct the dependency, then let a reviewed application/reconciler path create deterministic work;
   - application defect: deploy a fixed release first, then generate a new domain action from PostgreSQL authority;
   - already completed/duplicate: close as idempotently satisfied; do not republish.

8. There is no supported raw replay or purge command in this repository. If a future replay tool is implemented, require dry-run, deterministic identity, bounded selection, authorization, and audit output before using it.

## Verification

Verify all hosted streams show three current replicas and a leader, both consumers are active with decreasing pending/ack-pending, outbox oldest age converges, worker leases/retries normalize, projection lag converges, DLQ stops growing, and `/health/dependencies` reflects recovery. Do not require DLQ depth zero if records are intentionally retained.

## Rollback or safe stop

Stop any maintenance/replay when quorum is below contract, identities do not correlate, or data authority is unclear. Revert only through a reviewed GitOps lock/change. Preserve DLQ records and database state; never purge to silence an alert. If a corrected configuration worsens quorum, restore the prior compatible GitOps release.

## Escalation

Escalate cluster/storage/quorum faults to platform and NATS support; repeated application failures to service owners; data ambiguity to the data owner; suspected credential compromise to security; production core impact to the incident commander.

## Evidence and audit capture

Retain incident/approvals, GitOps release, pod/node/AZ/PVC state, bounded redacted NATS reports, consumer counters, DLQ identities/reasons (not secrets), database correlation, remediation PR/release, convergence metrics, and retained/unresolved record owners.
