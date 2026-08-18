# Observability module

Creates customer-KMS-encrypted Amazon Managed Prometheus and Amazon Managed Grafana workspaces, AMP logging, an IAM Identity Center-authenticated Grafana read role, retained encrypted CloudWatch log groups, an ADOT write policy, regional X-Ray KMS encryption, an X-Ray group, and sampling rules.

Use `7`, `30`, and `90` days for development, staging, and production application log retention unless a longer security/audit requirement applies. The caller supplies each log group explicitly, including the required `amp` group, so retention is reviewable rather than infinite.

OpenTofu owns the AWS resources only. GitOps owns the ADOT collector and its Kubernetes ServiceAccount. Attach `adot_policy_arn` to that account's role through the pod-identity module, configure SigV4 remote write to `prometheus_endpoint`, and export OTLP traces to X-Ray. Configure the Grafana Prometheus data source with the output endpoint and AWS SigV4; no API keys or generated Grafana credentials are stored in state.

Apply-time prerequisites include KMS policies for `aps`, Grafana, regional CloudWatch Logs, and X-Ray; IAM Identity Center enabled in the region with user IDs resolved; permissions to create the Grafana service role; and only one owner for the account/region X-Ray encryption configuration. Users cannot sign in until role associations exist. Dashboard and alert-rule provisioning remains a separate reviewed observability delivery step.
