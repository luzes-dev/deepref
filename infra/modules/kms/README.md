# KMS module

Creates purpose-specific customer-managed KMS keys with rotation enabled, explicit aliases, and a 7–30 day deletion waiting period. The account root retains IAM delegation, while optional administrator, service, and workload principals receive scoped administration or cryptographic use.

Create separate logical keys for EKS envelope encryption, RDS, ECR, Secrets Manager, and logs. Do not reuse state-backend keys here: state bootstrapping has a separate lifecycle and trust boundary.
