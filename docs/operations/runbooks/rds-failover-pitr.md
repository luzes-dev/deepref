# Runbook: RDS failover and point-in-time recovery

## Purpose and scope

Validate or recover production PostgreSQL from an instance/AZ failure using Multi-AZ failover, or restore an isolated new instance to an approved point in time. The target is RPO no more than five minutes and RTO no more than sixty minutes; regional loss is excluded.

## Safety warnings

- `reboot-db-instance --force-failover` is disruptive and production-only behavior.
- PITR creates a new database. Never restore over, delete, rename, or disable protection on the source first.
- Keep the restored database private and validate data, schema, jobs, and graph metrics rather than only instance availability.

## Prerequisites and authorization

Approved incident/drill, data/platform owners, protected AWS role, current `LatestRestorableTime`, healthy backups, private validation network, cutover plan, and evidence store.

## Triggers and symptoms

RDS instance/AZ failure, connection outage, logical corruption requiring recovery, or scheduled Multi-AZ/PITR acceptance drill.

## Ordered steps

1. Capture caller, source metadata, start time, backup state, private networking, and clocks.
2. For failover, perform one approved forced failover and wait for availability.
3. For PITR, restore a new private instance at the approved timestamp with the source subnet/security groups.
4. From the quarantined validator, verify schema, critical row/count invariants, newest durable transaction, queued/running/dead jobs, UUID graph facts, metric snapshots, and application behavior.
5. Prepare a reviewed OpenTofu/secret/GitOps cutover if recovery is accepted; preserve the source.
6. Verify API, worker lease recovery, ingestion durability, graph freshness, and alerts.

## Verification

Record recovery time and observed RTO. For PITR, compare the newest accepted transaction with the failure/restore target for observed RPO. Verify RDS private/deletion-protected posture, `/api/health/ready`, `/api/health/dependencies`, and durable job convergence.

## Rollback or safe stop

Stop after one failover and await AWS. Keep an invalid PITR quarantined and preserve the source. Cancel cutover before changing ownership; cleanup is a separate approved retention action.

## Escalation

Escalate RDS/restore failures to AWS/platform, data ambiguity to the data owner and incident commander, suspected malicious deletion to security, and target breach to leadership.

## Evidence and audit capture

Retain authorization, caller/account, source/target IDs, redacted descriptions, AWS events, recovery/restore time, clock definitions, validation results, cutover approvals, health/job evidence, alerts, and cleanup decision.
