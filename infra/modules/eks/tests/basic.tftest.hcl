mock_provider "aws" {}

run "private_three_az_cluster" {
  command = plan

  variables {
    name = "test-cluster"
    private_subnets_by_az = {
      sa-east-1a = "subnet-a"
      sa-east-1b = "subnet-b"
      sa-east-1c = "subnet-c"
    }
    cluster_kms_key_arn           = "arn:aws:kms:sa-east-1:111111111111:key/eks"
    control_plane_log_kms_key_arn = "arn:aws:kms:sa-east-1:111111111111:key/logs"
    node_volume_kms_key_arn       = "arn:aws:kms:sa-east-1:111111111111:key/eks"
    access_entries = {
      administrator = {
        principal_arn = "arn:aws:iam::111111111111:role/test-eks-admin"
        access_policy_arns = [
          "arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy",
        ]
      }
    }
    stateful_node_count    = 3
    stateless_min_size     = 3
    stateless_desired_size = 3
    stateless_max_size     = 9
  }

  assert {
    condition     = aws_eks_cluster.this.vpc_config[0].endpoint_private_access && !aws_eks_cluster.this.vpc_config[0].endpoint_public_access
    error_message = "The Kubernetes API must be private-only."
  }

  assert {
    condition     = length(aws_eks_node_group.stateful) == 3
    error_message = "The production topology must create one fixed stateful node in each AZ."
  }

  assert {
    condition     = aws_eks_node_group.stateless.scaling_config[0].max_size > aws_eks_node_group.stateless.scaling_config[0].min_size
    error_message = "Stateless capacity must be autoscalable."
  }

  assert {
    condition     = contains(aws_eks_cluster.this.enabled_cluster_log_types, "audit")
    error_message = "EKS audit logging must be enabled."
  }
}
