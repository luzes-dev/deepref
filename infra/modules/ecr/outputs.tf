output "repository_arns" {
  description = "Repository ARNs keyed by artifact name."
  value       = { for name, repository in aws_ecr_repository.this : name => repository.arn }
}

output "repository_urls" {
  description = "Repository URLs keyed by artifact name."
  value       = { for name, repository in aws_ecr_repository.this : name => repository.repository_url }
}

output "promotion_role_arn" {
  description = "Cross-account OCI promotion role ARN, or null when disabled."
  value       = try(aws_iam_role.promotion[0].arn, null)
}
