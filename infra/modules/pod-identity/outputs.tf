output "role_arns" {
  description = "Pod Identity role ARNs keyed by workload."
  value       = { for workload, role in aws_iam_role.this : workload => role.arn }
}

output "association_ids" {
  description = "EKS Pod Identity association IDs keyed by workload."
  value       = { for workload, association in aws_eks_pod_identity_association.this : workload => association.association_id }
}

output "service_accounts" {
  description = "Expected namespace and ServiceAccount names for the separate Kubernetes/GitOps owner."
  value = {
    for workload, association in var.associations : workload => {
      namespace       = association.namespace
      service_account = association.service_account
    }
  }
}
