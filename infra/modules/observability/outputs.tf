output "prometheus_workspace_id" {
  description = "Amazon Managed Prometheus workspace ID."
  value       = aws_prometheus_workspace.this.id
}

output "prometheus_workspace_arn" {
  description = "Amazon Managed Prometheus workspace ARN."
  value       = aws_prometheus_workspace.this.arn
}

output "prometheus_endpoint" {
  description = "Amazon Managed Prometheus endpoint used by ADOT and Grafana."
  value       = aws_prometheus_workspace.this.prometheus_endpoint
}

output "grafana_workspace_id" {
  description = "Amazon Managed Grafana workspace ID."
  value       = aws_grafana_workspace.this.id
}

output "grafana_endpoint" {
  description = "Amazon Managed Grafana workspace endpoint."
  value       = aws_grafana_workspace.this.endpoint
}

output "adot_policy_arn" {
  description = "IAM policy ARN to attach to the ADOT collector's EKS Pod Identity role."
  value       = aws_iam_policy.adot.arn
}

output "log_group_arns" {
  description = "Encrypted CloudWatch log group ARNs keyed by logical name."
  value       = { for name, log_group in aws_cloudwatch_log_group.this : name => log_group.arn }
}

output "xray_group_arn" {
  description = "X-Ray trace group ARN."
  value       = aws_xray_group.this.arn
}
