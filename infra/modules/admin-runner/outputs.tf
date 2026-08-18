output "project_name" {
  description = "CodeBuild project name used for approved manual starts."
  value       = aws_codebuild_project.this.name
}

output "project_arn" {
  description = "CodeBuild administration runner ARN."
  value       = aws_codebuild_project.this.arn
}

output "role_arn" {
  description = "Runner IAM role ARN; add this as an explicit EKS access entry."
  value       = aws_iam_role.this.arn
}

output "security_group_id" {
  description = "Egress-only runner security group ID."
  value       = aws_security_group.this.id
}

output "log_group_arn" {
  description = "Encrypted runner log group ARN."
  value       = aws_cloudwatch_log_group.this.arn
}
