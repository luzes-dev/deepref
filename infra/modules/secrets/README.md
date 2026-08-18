# Secrets module

Creates encrypted Secrets Manager containers and optional least-privilege resource policies. This module deliberately has no secret-value input and never creates a `aws_secretsmanager_secret_version`; operators or an approved rotation workflow populate values out of band.

Deletion uses a recovery window, public resource policies are rejected, and optional regional replicas can use region-specific KMS keys. Pass only workload role ARNs that require `DescribeSecret` and `GetSecretValue`.
