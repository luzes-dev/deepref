# Runbook: node or availability-zone drain

## Purpose and scope

Safely drain one Kubernetes node or AZ while preserving API, worker, web, telemetry, Cloudflare Tunnel, RDS connectivity, and durable PostgreSQL jobs.

## Safety warnings

- One node at a time; never bypass PDBs or use `--force`/`--disable-eviction` for an unknown blocker.
- Freeze promotions, migrations, restores, and unrelated maintenance.
- Do not manually change managed node-group capacity without reconciling source.

## Prerequisites and authorization

Approved maintenance, incident commander/platform owner, capacity for rescheduling, current Argo/health/job baseline, and access to the private cluster.

## Triggers and symptoms

Node maintenance, AZ capacity issue, health degradation, security remediation, or planned node-group update.

## Ordered steps

1. Record authorization, node/AZ, node group, pods/PDBs, RDS status, API/worker/web health, queue age, and telemetry baseline.
2. Cordon the node and confirm replacement capacity.
3. Drain with eviction respecting PDBs:

   ```bash
   kubectl cordon "$NODE"
   kubectl drain "$NODE" --ignore-daemonsets --timeout=20m
   ```

4. If blocked only by approved scratch data, obtain explicit approval before using `--delete-emptydir-data`; do not add `--force`.
5. Wait for rescheduled workloads to become ready and for job claims/lease recovery to settle before draining another node.
6. Complete the underlying managed-node/OpenTofu maintenance through its owner.
7. Uncordon the node when it is healthy and intended to return.

## Verification

Verify nodes Ready, pods ready across AZs, PDBs respected, RDS healthy, API/worker/web health, queue age and recovery, graph metric freshness, telemetry, Cloudflare Tunnel, Argo sync, and no volume loss.

## Rollback or safe stop

If eviction blocks or health degrades, stop, uncordon if safe, and allow workloads to stabilize. Revert capacity/configuration through OpenTofu/GitOps. Do not continue the AZ sequence on a shortfall.

## Escalation

Escalate capacity/PDB issues to platform, RDS symptoms to the data/platform owner, API/worker/data symptoms to service owners, and production target impact to the incident commander.

## Evidence and audit capture

Retain authorization, node/AZ and node-group inventory, pod/PDB/volume snapshots, before/after health and queue baselines, command/timestamps, approvals, alerts, Argo revision, and recovery confirmation.
