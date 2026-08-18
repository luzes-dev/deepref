mock_provider "aws" {}

run "private_ephemeral_admin_runner" {
  command = plan

  variables {
    name             = "ambient-scribes-test-admin"
    vpc_id           = "vpc-00000000000000000"
    subnet_ids       = ["subnet-00000000000000001", "subnet-00000000000000002"]
    eks_cluster_name = "ambient-scribes-test"
    eks_cluster_arn  = "arn:aws:eks:sa-east-1:111111111111:cluster/ambient-scribes-test"
    log_kms_key_arn  = "arn:aws:kms:sa-east-1:111111111111:key/00000000-0000-0000-0000-000000000000"
  }

  assert {
    condition     = aws_codebuild_project.this.vpc_config[0].vpc_id == "vpc-00000000000000000"
    error_message = "The runner must be VPC connected."
  }

  assert {
    condition     = aws_codebuild_project.this.source[0].type == "NO_SOURCE" && aws_codebuild_project.this.artifacts[0].type == "NO_ARTIFACTS"
    error_message = "The administration runner must not persist source or artifacts."
  }

  assert {
    condition     = !aws_codebuild_project.this.environment[0].privileged_mode
    error_message = "The administration runner must not use privileged containers."
  }

  assert {
    condition     = length(aws_security_group.this.ingress) == 0
    error_message = "The runner security group must have no inbound rules."
  }
}
