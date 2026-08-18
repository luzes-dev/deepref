output "cluster_name" {
  description = "EKS cluster name."
  value       = aws_eks_cluster.this.name
}

output "cluster_arn" {
  description = "EKS cluster ARN."
  value       = aws_eks_cluster.this.arn
}

output "cluster_endpoint" {
  description = "Private Kubernetes API endpoint."
  value       = aws_eks_cluster.this.endpoint
}

output "cluster_certificate_authority_data" {
  description = "Base64-encoded Kubernetes cluster CA data."
  value       = aws_eks_cluster.this.certificate_authority[0].data
  sensitive   = true
}

output "cluster_primary_security_group_id" {
  description = "EKS-created cluster security group used by nodes and control plane."
  value       = aws_eks_cluster.this.vpc_config[0].cluster_security_group_id
}

output "node_role_arn" {
  description = "IAM role shared by managed node groups."
  value       = aws_iam_role.node.arn
}

output "ebs_csi_role_arn" {
  description = "Pod Identity IAM role for the EBS CSI controller."
  value       = aws_iam_role.ebs_csi.arn
}

output "stateful_node_group_names" {
  description = "Fixed stateful node group names keyed by AZ."
  value       = { for zone, group in aws_eks_node_group.stateful : zone => group.node_group_name }
}

output "stateless_node_group_name" {
  description = "Autoscaled stateless node group name."
  value       = aws_eks_node_group.stateless.node_group_name
}
