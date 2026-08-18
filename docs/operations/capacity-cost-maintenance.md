# Capacity, cost, and maintenance

## Capacity envelope

The initial design envelope is tens of team users, 250,000 works, and 2.5 million citation edges. This is a planning limit, not measured production capacity. The ignored projector rebuild performance test generates that representative graph and targets completion under sixty minutes; a real deployed rebuild is still required.

Current source defaults include:

- EKS 1.36, three AZs, fixed stateful nodes and autoscaled stateless node groups.
- Development: one NAT gateway, one stateful node, small stateless range, `db.t4g.medium`, 30–100 GiB RDS autoscaling range.
- Staging: one NAT per AZ, three stateful nodes, `db.r7g.large`, 100–500 GiB RDS range.
- Production: one NAT per AZ, three stateful `m7g.xlarge` nodes, stateless desired 6/range 3–18, `db.r7g.xlarge`, 200–1000 GiB RDS range.
- Production chart fixtures use three API/worker/projector/web replicas, three NATS replicas, two fixed tunnel replicas, and one Neo4j instance.

These are source contracts only. Verify quotas, instance availability, ARM64 image support, requested/actual node and pod capacity, storage class behavior, and cost before apply.

## Capacity review

Review at an agreed cadence and before promotions that materially change load:

1. API rate, p95/p99 latency, CPU/memory throttling, replica availability, and saturation.
2. Worker queue pending, processing duration, retry/lease recovery, Crossref permit wait, and outbox age.
3. Projection lag, throughput, failures, Neo4j heap/page cache/storage, and metric snapshot age.
4. RDS CPU, connections, free storage, I/O/latency, lock waits, autoscaling headroom, and Performance Insights.
5. NATS stream bytes/messages, file store, consumer pending/ack pending, replica health, and DLQ.
6. EKS pending pods, node/volume/AZ balance, PDB constraints, and stateless autoscaling limits.
7. Tunnel replicas/certificates and synthetic latency.

Capacity change goes through source review: chart values/release lock for workload resources and replicas; OpenTofu for nodes, RDS, network, and managed services. Do not use ad-hoc `kubectl scale` as the normal response.

## Cost controls

The `infra/modules/budgets-alerts` module defines a KMS-encrypted SNS topic, AWS Budget notifications, and CloudWatch alarms, while `observability/alerts/budget.yaml` defines cost-signal rules. The environment roots do not currently instantiate that module, so budgets/SNS are not established by the present roots.

Before production:

- approve per-account monthly budgets, warning/forecast thresholds, CostCenter/Owner tags, and a finance owner;
- wire/apply the module and confirm every SNS subscription;
- verify budget alerts end-to-end and identify Cloudflare cost visibility;
- decide approval thresholds for capacity increases and emergency cost exceptions.

An AWS Budget sends notifications; it does not cap spend. Missing cost metrics or an unconfirmed subscription is a control failure.

## Maintenance windows

RDS module defaults are automated backup `03:00-04:00 UTC` and maintenance `sun:05:00-sun:06:00 UTC`; production source uses automatic minor upgrades and `apply_immediately = false`. Validate that these windows are acceptable in `America/Sao_Paulo` at apply time and record seasonal/local-time communication expectations.

Maintenance classes:

| Class                       | Required path                                                                        |
| --------------------------- | ------------------------------------------------------------------------------------ |
| Application/chart           | Build-once release and environment promotion                                         |
| Kubernetes/controller/chart | OpenTofu/Helm source review, compatibility test, staged rollout                      |
| EKS version/nodes           | OpenTofu plan, staging node/AZ drain drill, production window                        |
| RDS version/class/storage   | OpenTofu plan, current backup/PITR evidence, staging validation, data-owner approval |
| NATS                        | GitOps chart/config change, quorum and consumer-lag verification                     |
| Neo4j                       | GitOps image/config change, rebuild readiness and graph-degradation verification     |
| Credentials/certificates    | Credential-rotation runbook with overlap and audit evidence                          |
| Third-party images          | Mirror/pin, scan/sign/attest, chart/release promotion                                |

## Node and AZ maintenance

Use [node/AZ drain](runbooks/node-az-drain.md). PDBs protect replicated workloads; Neo4j intentionally has no PDB because a singleton `minAvailable: 1` would make voluntary drain impossible. A Neo4j eviction causes graph-only degradation and may require a rebuild. Never drain multiple NATS stateful nodes or enough nodes to lose JetStream quorum.

## Upgrade safety

- Confirm vendor support and version skew before changing EKS, Kubernetes clients, Argo, NATS, Neo4j, controllers, or PostgreSQL.
- Mirror and digest-pin third-party runtime/controller images in ECR before hosted use.
- Test Helm rendering/policy and OpenTofu plans; stage the exact change before production.
- Define rollback compatibility. Database engine and schema downgrades are not ordinary rollback paths.
- Freeze unrelated releases during risky maintenance and capture baseline/after metrics.

## Evidence

Retain the approved capacity hypothesis, dashboard snapshots/query exports, cost estimate and budget status, plan/PR/workflow identities, maintenance timeline, drain/quorum checks, before/after health, alert behavior, and any revised limit. Do not mark the initial envelope accepted solely from configuration defaults.
