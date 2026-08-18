# Backup, restore, and disaster recovery

## Recovery model

PostgreSQL is the recovery authority. Production OpenTofu source configures RDS PostgreSQL 17 as private, encrypted, Multi-AZ, deletion-protected, with 35 days of automated-backup/PITR retention. Development and staging are intentionally Single-AZ and not deletion-protected. Neo4j is a rebuildable graph projection; NATS carries durable work/domain/DLQ events but is not the system of record.

The declared production target is PostgreSQL RPO no more than five minutes and RTO no more than sixty minutes for service or AZ failures. Total loss of `sa-east-1` is outside that target. Before production, the data owner must define clock start/stop, evidence source, drill frequency, and which failure classes qualify; do not claim the target until RDS failover and PITR drills meet it.

## Current source versus active protection

- `infra/modules/rds` encodes automated backups, maintenance windows, encryption, private networking, Performance Insights, and production safeguards.
- `infra/modules/backup` encodes an encrypted Backup Vault, Vault Lock, scheduled recovery points, and restore role.
- The per-environment roots instantiate the reusable backup module against the environment RDS instance, with environment-specific retention of 7, 14, and 35 days. This is source wiring, not proof that a vault or recovery point exists.
- No apply output, recovery-point inventory, restored-instance proof, or measured RPO/RTO is present.

Reviewing the retention/compliance mode, applying each root, and completing the failover/PITR drills remains a production blocker.

## Backup verification

After deployment, use the environment root output and AWS APIs rather than guessed identifiers:

```bash
export DB_INSTANCE_ID="ambient-scribes-${ENVIRONMENT}"

aws sts get-caller-identity
aws rds describe-db-instances \
  --region "$AWS_REGION" \
  --db-instance-identifier "$DB_INSTANCE_ID" \
  --query 'DBInstances[0].{Status:DBInstanceStatus,MultiAZ:MultiAZ,DeletionProtection:DeletionProtection,Retention:BackupRetentionPeriod,Latest:LatestRestorableTime,Public:PubliclyAccessible}'
aws rds describe-db-snapshots \
  --region "$AWS_REGION" \
  --db-instance-identifier "$DB_INSTANCE_ID" \
  --snapshot-type automated
```

Verify the exact `backup_vault_arn` root output and recovery points. Do not infer active protection from source alone.

## Restore principles

1. Restore to a new isolated RDS instance. Never overwrite, rename, or delete the source as the first action.
2. Use an approved restore timestamp and capture `LatestRestorableTime` before starting.
3. Attach only approved private subnet/security groups. A restored database must remain non-public.
4. Validate schema, row/count invariants, sampled hashes, and application behavior from a quarantined validation client.
5. Decide cutover through a reviewed configuration/GitOps change. Do not patch live Secrets or Deployments as the normal path.
6. Preserve the original instance until the data owner accepts the restore and the retention period permits cleanup.

Follow [RDS failover and PITR](runbooks/rds-failover-pitr.md) for concrete commands.

## Service and AZ failure

For a production Multi-AZ database, use the RDS failover drill only under an approved maintenance/incident record. Observe application readiness, reconnect behavior, ingestion durability, and recovery clocks. Do not force failover in development/staging and extrapolate it as production proof.

During database unavailability, stop promotions and migrations. Preserve queue state; do not purge NATS or rebuild Neo4j until PostgreSQL authority is healthy. After PostgreSQL recovery, verify worker claims/outbox, projection lag, and graph status before clearing the incident.

## Neo4j and NATS recovery order

- If Neo4j alone fails, core workflows remain available and graph routes degrade with `GRAPH_UNAVAILABLE`. Restore storage only when trustworthy; otherwise run the approved rebuild from PostgreSQL.
- If NATS quorum fails, stabilize quorum and inspect stream/consumer state before replaying DLQ records. PostgreSQL reconciliation repairs expired claims and missing deterministic work events.
- If both fail with PostgreSQL healthy, restore NATS delivery first, then let projection catch up or rebuild Neo4j.
- If PostgreSQL is suspect, treat NATS and Neo4j as secondary evidence only. Do not promote either to authority.

## Recovery acceptance and evidence

For each drill retain failure injection authorization, pre-drill backup/PITR state, UTC clock definitions, failover/restore API events, new instance identity, connectivity controls, observed RPO/RTO, database invariants, application/worker/projector health, alarms and notifications, cutover/safe-stop decision, approvers, and cleanup disposition.

Evidence must be redacted. Database snapshots, exports, SQL dumps, connection URLs, and Secrets Manager contents do not belong in Git or workflow artifacts.
