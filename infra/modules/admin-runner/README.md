# Administration runner module

Creates an on-demand CodeBuild project attached to private VPC subnets, an egress-only security group, an encrypted retained log group, and a dedicated IAM role. It has no source repository, artifacts, inbound listener, static credentials, or secret environment variables. The default build only verifies access to the private EKS API.

Add the output role ARN to the cluster's explicit EKS access entries with only the Kubernetes permissions required for bootstrap or break-glass work. IAM permission to call the EKS API does not itself grant Kubernetes authorization. Restrict who may call `codebuild:StartBuild`, especially with buildspec/environment overrides, and audit every build through CloudTrail and the log group.

Apply-time prerequisites are private subnets with DNS and routes/VPC endpoints for EKS, STS, CloudWatch Logs, ECR/S3 as needed; a log KMS policy permitting the regional Logs service; an approved CodeBuild image containing AWS CLI and kubectl; and an EKS access entry for the runner role. Where endpoints permit, replace the default broad egress CIDR with private approved destinations.
