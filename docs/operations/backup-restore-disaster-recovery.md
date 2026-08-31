# Backup, restore, and disaster recovery

## Recovery model

PostgreSQL is the recovery authority for application state, durable jobs, graph facts, and metric snapshots. Production OpenTofu configures RDS PostgreSQL 17 as private, encrypted, Multi-AZ, deletion-protected, with 35 days of automated-backup/PITR retention. Development and staging are intentionally Single-AZ and not deletion-protected.

The declared production target is PostgreSQL RPO no more than five minutes and RTO no more than sixty minutes for service or AZ failures. Total loss of `sa-east-1` is outside that target. Do not claim the target until failover and PITR drills meet it.

## Current source versus active protection

- `infra/modules/rds` encodes automated backups, maintenance windows, encryption, private networking, Performance Insights, and production safeguards.
- `infra/modules/backup` encodes an encrypted Backup Vault, Vault Lock, scheduled recovery points, and restore role.
- Per-environment roots currently instantiate RDS but do not instantiate the reusable backup module. Backup Vault/Vault Lock is source implemented but not connected or deployable from those roots yet.
- No apply output, recovery-point inventory, restored-instance proof, or measured RPO/RTO is present.

Wiring the backup module into each intended root, reviewing its retention/compliance mode, and applying it is a production blocker.

## Backup verification

After deployment, use the environment root output and AWS APIs rather than guessed identifiers:

```bash
aws sts get-caller-identity
aws rds describe-db-instances --region "$AWS_REGION" \
  --db-instance-identifier "ambient-scribes-${ENVIRONMENT}" \
  --query 'DBInstances[0].{Status:DBInstanceStatus,MultiAZ:MultiAZ,DeletionProtection:DeletionProtection,Retention:BackupRetentionPeriod,Latest:LatestRestorableTime,Public:PubliclyAccessible}'
aws rds describe-db-snapshots --region "$AWS_REGION" \
  --db-instance-identifier "ambient-scribes-${ENVIRONMENT}" --snapshot-type automated
```

## Restore principles

1. Restore to a new isolated RDS instance. Never overwrite the source as the first action.
2. Use an approved restore timestamp and capture `LatestRestorableTime` before starting.
3. Attach only approved private subnet/security groups.
4. Validate schema, row/count invariants, sampled hashes, queued/running/dead job state, and application behavior from a quarantined client.
5. Decide cutover through a reviewed OpenTofu/secret/GitOps change.
6. Preserve the original instance until the data owner accepts the restore.

Follow [RDS failover and PITR](runbooks/rds-failover-pitr.md) for concrete commands.

## Service and AZ failure

Use the RDS failover drill only under an approved maintenance or incident record. Observe API readiness, worker lease recovery, ingestion durability, and recovery clocks. Do not extrapolate development/staging behavior as production proof.

## Evidence

Retain caller/account, source/target identifiers, redacted RDS descriptions, recovery point and restore time, clock definitions, validation queries, cutover approvals, health/job evidence, and the cleanup decision.
