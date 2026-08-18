# Private EKS module

Creates a private-only EKS 1.36 cluster with API-mode access entries, all control-plane logs (including audit), Kubernetes secret envelope encryption, EBS CSI, and EKS Pod Identity. It intentionally exposes no public Kubernetes API endpoint.

Stateful capacity is one fixed managed node group per selected AZ. Set `stateful_node_count = 1` in development and `3` in staging/production to guarantee the required fixed topology. Stateful nodes are tainted `dedicated=stateful:NoSchedule`. A separate on-demand stateless group spans all three private subnets and scales between explicit minimum and maximum bounds; Karpenter is not installed.

The node role intentionally excludes SSM and workload permissions. A later least-privilege IAM/support slice can add Pod Identity associations for workloads and a separately controlled administration path.
