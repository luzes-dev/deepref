# Network module

Creates a three-AZ VPC with public ingress subnets, private workload subnets, and isolated data subnets. Workloads use either one shared NAT gateway (`single`) or one NAT gateway in every AZ (`one_per_az`). Data subnets have no default internet route.

Private endpoints cover ECR API/Docker, S3, STS, CloudWatch Logs, Secrets Manager, and the SSM control/data channels. VPC flow logs are retained in an optionally KMS-encrypted CloudWatch log group.

## Contract

- Supply three distinct AZs and three non-overlapping CIDRs per subnet tier.
- Use `single` only for development; staging and production should use `one_per_az`.
- Pass the logs KMS key ARN through `flow_log_kms_key_arn`.
- Consume `private_subnet_ids` for EKS and `data_subnet_ids` for RDS.
