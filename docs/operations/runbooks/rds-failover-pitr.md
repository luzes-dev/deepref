# Runbook: RDS failover and point-in-time recovery

## Purpose and scope

Validate or recover production PostgreSQL from an instance/AZ failure using Multi-AZ failover, or restore an isolated new instance to an approved point in time. The target is RPO no more than five minutes and RTO no more than sixty minutes for service/AZ failures; regional loss is excluded.

## Safety warnings

- `reboot-db-instance --force-failover` is disruptive and production-only Multi-AZ behavior. Run it only as an approved drill or incident mitigation.
- PITR creates a new database. Never restore over, delete, rename, or disable protection on the source as the first action.
- Never make the restored database public or attach unreviewed security groups/subnets.
- Do not infer data correctness from `available`; validate schema, counts/hashes, transactions, and application behavior.
- RPO/RTO clocks and requested restore time must be recorded before action.

## Prerequisites and authorization

- Incident or approved production recovery drill with incident commander, data owner, platform operator, UTC clock observer, and protected AWS role.
- Caller/account verified, source identifier `ambient-scribes-production`, current `LatestRestorableTime`, healthy automated backups, and sufficient RDS quota/storage/IP capacity.
- Approved RPO/RTO clock definitions, validation queries, quarantine network, cutover plan, rollback/safe-stop authority, and evidence store.
- Promotions, migrations, drains, and unrelated maintenance frozen.

## Triggers and symptoms

- RDS instance/AZ failure, connection outage, or AWS failover event.
- Suspected logical data corruption/deletion requiring recovery to a pre-event timestamp.
- Scheduled Multi-AZ failover or PITR acceptance drill.

## Ordered steps

1. Establish identity, source metadata, and clocks:

   ```bash
   export AWS_REGION=sa-east-1
   export DB_INSTANCE_ID=ambient-scribes-production
   export STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
   aws sts get-caller-identity
   aws rds describe-db-instances --region "$AWS_REGION" \
     --db-instance-identifier "$DB_INSTANCE_ID" > /tmp/deepref-rds-before.json
   jq '.DBInstances[0] | {DBInstanceStatus,MultiAZ,DeletionProtection,BackupRetentionPeriod,LatestRestorableTime,DBSubnetGroup,VpcSecurityGroups}' \
     /tmp/deepref-rds-before.json
   ```

2. Confirm `MultiAZ=true`, `DeletionProtection=true`, retention `35`, private networking, the approved incident/drill, and the exact selected path.

3. **Failover path only**: announce the disruption and force one controlled failover:

   ```bash
   aws rds reboot-db-instance --region "$AWS_REGION" \
     --db-instance-identifier "$DB_INSTANCE_ID" \
     --force-failover
   aws rds wait db-instance-available --region "$AWS_REGION" \
     --db-instance-identifier "$DB_INSTANCE_ID"
   ```

   Observe API readiness, reconnects, worker leases/outbox, and RDS events continuously. Do not issue a second failover to “unstick” the first.

4. **PITR path only**: record an approved RFC3339 `RESTORE_TIME` not later than `LatestRestorableTime`. Derive the original private subnet group and security group IDs from the captured description:

   ```bash
   export RESTORE_TIME=REPLACE_WITH_APPROVED_RFC3339_UTC
   export TARGET_DB_INSTANCE_ID="ambient-scribes-production-pitr-$(date -u +%Y%m%d%H%M%S)"
   export DB_SUBNET_GROUP="$(jq -r '.DBInstances[0].DBSubnetGroup.DBSubnetGroupName' /tmp/deepref-rds-before.json)"
   mapfile -t DB_SECURITY_GROUPS < <(jq -r '.DBInstances[0].VpcSecurityGroups[].VpcSecurityGroupId' /tmp/deepref-rds-before.json)

   aws rds restore-db-instance-to-point-in-time \
     --region "$AWS_REGION" \
     --source-db-instance-identifier "$DB_INSTANCE_ID" \
     --target-db-instance-identifier "$TARGET_DB_INSTANCE_ID" \
     --restore-time "$RESTORE_TIME" \
     --db-subnet-group-name "$DB_SUBNET_GROUP" \
     --vpc-security-group-ids "${DB_SECURITY_GROUPS[@]}" \
     --no-publicly-accessible \
     --deletion-protection
   aws rds wait db-instance-available --region "$AWS_REGION" \
     --db-instance-identifier "$TARGET_DB_INSTANCE_ID"
   ```

5. From the quarantined private validator, use approved temporary credentials to verify TLS, schema version, critical row/count invariants, newest durable transaction, sampled hashes, and absence/presence of the incident transaction at the selected time. Do not connect production workloads yet.

6. For actual recovery, prepare a reviewed OpenTofu/secret/GitOps cutover that preserves the original instance. Because the current roots own a specific RDS resource, do not point Terraform at the restored instance or edit runtime Secrets ad hoc without an import/configuration plan and data-owner approval.

7. After cutover, verify API, worker/outbox, NATS delivery, projection lag, and graph behavior. A Neo4j rebuild may follow only after PostgreSQL is accepted as authoritative.

## Verification

Record `RECOVERED_AT` when the agreed core service and data checks pass, and compute observed RTO from `STARTED_AT`. For PITR, compare the newest accepted durable transaction timestamp to the failure/restore target for observed RPO. Verify RDS events, private/deletion-protected posture, `/health/ready`, `/health/dependencies`, ingestion durability, and alerts.

## Rollback or safe stop

For failover, stop after one API request and await AWS; do not toggle AZs repeatedly. For PITR, keep the restored instance quarantined if validation fails and preserve the source. Cancel cutover before changing ownership. Cleanup of a deletion-protected restored instance is a later, separately approved data-retention change—never an automatic runbook step.

## Escalation

Escalate stalled/unavailable RDS or restore failures to AWS Support/platform owner; data ambiguity to the data owner and incident commander; suspected malicious deletion to security; an approaching target breach to leadership/communications.

## Evidence and audit capture

Retain authorization, caller/account, source/target identifiers, redacted before/after descriptions, AWS events, recovery-point/restore time, clock definitions and observed RPO/RTO, validation queries/results, cutover plan/approvals, health/queue/projection evidence, alerts, and cleanup decision. Delete `/tmp` metadata per runner policy; it must contain no credentials.
