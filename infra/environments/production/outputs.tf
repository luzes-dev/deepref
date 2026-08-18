output "vpc_id" {
  description = "Production VPC ID."
  value       = module.network.vpc_id
}

output "private_subnet_ids" {
  description = "Production private workload subnet IDs."
  value       = module.network.private_subnet_ids
}

output "eks_cluster_name" {
  description = "Private production EKS cluster name."
  value       = module.eks.cluster_name
}

output "rds_endpoint" {
  description = "Production PostgreSQL endpoint."
  value       = module.rds.endpoint
}

output "rds_master_user_secret_arn" {
  description = "AWS-managed database master credential secret ARN."
  value       = module.rds.master_user_secret_arn
  sensitive   = true
}

output "ecr_repository_urls" {
  description = "Production ECR repository URLs."
  value       = module.ecr.repository_urls
}

output "ecr_promotion_role_arn" {
  description = "Production OCI promotion role ARN."
  value       = module.ecr.promotion_role_arn
}

output "secret_arns" {
  description = "Metadata-only application secret container ARNs."
  value       = module.secrets.secret_arns
}

output "kms_key_arns" {
  description = "Purpose-specific platform KMS key ARNs."
  value       = module.kms.key_arns
}

output "backup_vault_arn" {
  description = "Encrypted, Vault-Locked AWS Backup vault ARN."
  value       = module.backup.vault_arn
}

output "prometheus_workspace_endpoint" {
  description = "Amazon Managed Prometheus remote-write endpoint."
  value       = module.observability.prometheus_endpoint
}

output "grafana_workspace_endpoint" {
  description = "Amazon Managed Grafana workspace endpoint."
  value       = module.observability.grafana_endpoint
}

output "admin_runner_project_name" {
  description = "On-demand private CodeBuild administration runner name."
  value       = module.admin_runner.project_name
}
