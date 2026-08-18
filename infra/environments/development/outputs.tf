output "vpc_id" {
  description = "Development VPC ID."
  value       = module.network.vpc_id
}

output "private_subnet_ids" {
  description = "Development private workload subnet IDs."
  value       = module.network.private_subnet_ids
}

output "eks_cluster_name" {
  description = "Private development EKS cluster name."
  value       = module.eks.cluster_name
}

output "rds_endpoint" {
  description = "Development PostgreSQL endpoint."
  value       = module.rds.endpoint
}

output "rds_master_user_secret_arn" {
  description = "AWS-managed database master credential secret ARN."
  value       = module.rds.master_user_secret_arn
  sensitive   = true
}

output "ecr_repository_urls" {
  description = "Development ECR repository URLs."
  value       = module.ecr.repository_urls
}

output "secret_arns" {
  description = "Metadata-only application secret container ARNs."
  value       = module.secrets.secret_arns
}

output "kms_key_arns" {
  description = "Purpose-specific platform KMS key ARNs."
  value       = module.kms.key_arns
}
