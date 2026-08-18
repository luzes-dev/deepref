# Runbook: node or AZ drain

## Purpose and scope

Safely drain one EKS node or the nodes in one AZ for planned maintenance while preserving replicated workloads and JetStream quorum and accepting the documented graph-only degradation risk of singleton Neo4j.

## Safety warnings

- Never drain more than one NATS stateful replica/node at a time or enough of a three-node cluster to lose two-of-three quorum.
- Neo4j has no PDB by design. Evicting its singleton may make graph/recommendation routes unavailable and may require rebuild; core must remain available.
- Do not use `--disable-eviction`, delete PVCs, force-delete stateful pods, or bypass PDBs to make a drain complete.
- `emptyDir` data is disposable runtime scratch only after confirmed workload design. Review pods before using `--delete-emptydir-data`.
- A whole-AZ drain requires capacity in the other AZs, volume topology review, and production change approval.

## Prerequisites and authorization

- Approved maintenance/incident, platform owner, service owner, and production change approval; data owner aware of Neo4j impact.
- Healthy EKS/Argo, all three NATS replicas current, RDS healthy/Multi-AZ in production, no migration/rebuild/restore/promotion, and enough spare stateless capacity.
- Node name/AZ/node group identified; rollback/un-cordon authority and monitoring established.
- For AZ drain, staging drill evidence and explicit ordered node list.

## Triggers and symptoms

- EKS managed node maintenance/replacement, kernel/runtime issue, impaired node, or AZ evacuation drill.
- Pod/node pressure requiring controlled evacuation.
- Never drain merely to clear a transient alert without diagnosing capacity and quorum.

## Ordered steps

1. Capture topology, workloads, PDBs, and volume placement:

   ```bash
   kubectl get nodes -L eks.amazonaws.com/nodegroup,topology.kubernetes.io/zone
   kubectl get pods --all-namespaces -o wide --field-selector spec.nodeName=REPLACE_WITH_NODE
   kubectl get pdb --all-namespaces
   kubectl get pvc,pv --namespace "$NAMESPACE" -o wide
   ```

2. Confirm RDS and application baseline:

   ```bash
   aws rds describe-db-instances --region "$AWS_REGION" \
     --db-instance-identifier "ambient-scribes-${ENVIRONMENT}" \
     --query 'DBInstances[0].{Status:DBInstanceStatus,MultiAZ:MultiAZ}'
   argocd app get deepref-root --refresh
   curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
   ```

3. Confirm NATS quorum/placement before every stateful-node drain:

   ```bash
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" server report jetstream
   nats --server "$NATS_URL" --creds "$NATS_OBSERVER_CREDS" --tlsca "$NATS_CA_FILE" stream report
   ```

4. Cordon exactly one node and verify no unexpected new pods land there:

   ```bash
   export NODE=REPLACE_WITH_EXACT_NODE_NAME
   kubectl cordon "$NODE"
   kubectl get node "$NODE"
   ```

5. Perform a normal eviction-respecting drain. First try without deleting `emptyDir` data:

   ```bash
   kubectl drain "$NODE" --ignore-daemonsets --timeout=20m
   ```

   If blocked only by reviewed application scratch `emptyDir`, obtain explicit platform approval and rerun with `--delete-emptydir-data`. Do not add `--force` or `--disable-eviction` to bypass an unknown blocker.

6. Wait for evicted workloads to become ready elsewhere. After any NATS pod move, require the stream replica set and consumers to be healthy before touching another node. Expect Neo4j graph degradation until its volume/pod is healthy.

7. Complete the underlying node-group/OpenTofu/AWS maintenance through its approved owner. Do not manually change managed node-group desired capacity without reconciling source.

8. When the node is healthy and intended to return, uncordon it:

   ```bash
   kubectl uncordon "$NODE"
   kubectl get node "$NODE" -o wide
   ```

9. For an AZ drain, repeat one node at a time, returning to full NATS quorum/application health between nodes. Stop before the next node on any shortfall.

## Verification

Verify expected nodes Ready, pods rescheduled/ready across AZs, PDBs respected, NATS all replicas/current and consumers progressing, RDS healthy, core readiness/synthetics pass, projection lag converges, Neo4j/graph recovers or enters the rebuild runbook, Argo is synced/healthy, and no PVC is lost.

## Rollback or safe stop

If eviction blocks or quorum/health degrades, stop, uncordon the node if safe, and allow workloads to stabilize. Do not continue the AZ sequence. Revert capacity/configuration through OpenTofu/GitOps. If Neo4j cannot recover, keep core available and use [Neo4j rebuild](neo4j-rebuild.md).

## Escalation

Escalate NATS quorum/PVC issues to platform/vendor support, Neo4j/data parity to graph/data owners, RDS/AZ symptoms to AWS Support, PDB/capacity defects to service/platform owners, and production target impact to the incident commander.

## Evidence and audit capture

Retain authorization, node/AZ/node group list, before/after topology, pod/PDB/PVC inventory, NATS/RDS/core/graph baselines, cordon/drain/un-cordon timestamps/commands, blockers/approvals, maintenance change identity, alerts, and recovery confirmation.
