mock_provider "aws" {}

run "roles_and_associations_only" {
  command = plan

  variables {
    cluster_name = "ambient-scribes-test"
    name_prefix  = "ambient-scribes-test"
    associations = {
      api = {
        namespace       = "platform"
        service_account = "api"
        inline_policy_json = jsonencode({
          Version = "2012-10-17"
          Statement = [{
            Effect   = "Allow"
            Action   = ["secretsmanager:GetSecretValue"]
            Resource = ["arn:aws:secretsmanager:sa-east-1:111111111111:secret:test"]
          }]
        })
      }
      adot = {
        namespace           = "observability"
        service_account     = "adot-collector"
        managed_policy_arns = ["arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess"]
      }
    }
  }

  assert {
    condition     = length(aws_iam_role.this) == 2 && length(aws_eks_pod_identity_association.this) == 2
    error_message = "Every workload must receive one IAM role and one Pod Identity association."
  }

  assert {
    condition     = jsondecode(aws_iam_role.this["api"].assume_role_policy).Statement[0].Principal.Service == "pods.eks.amazonaws.com"
    error_message = "Workload roles must trust only the EKS Pod Identity service."
  }

  assert {
    condition     = length(aws_iam_role_policy.inline) == 1
    error_message = "Only workloads with inline policy JSON should receive an inline policy."
  }
}
