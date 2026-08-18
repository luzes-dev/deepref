mock_provider "aws" {}

run "immutable_scanned_repositories" {
  command = plan

  variables {
    name_prefix = "test"
    kms_key_arn = "arn:aws:kms:sa-east-1:111111111111:key/ecr"
    repositories = {
      api   = { name = "test/api" }
      chart = { name = "test/charts/ambient-scribes" }
      web   = { name = "test/web" }
    }
  }

  assert {
    condition     = length(aws_ecr_repository.this) == 3
    error_message = "All declared artifact repositories must be created."
  }

  assert {
    condition     = alltrue([for repository in aws_ecr_repository.this : repository.image_tag_mutability == "IMMUTABLE"])
    error_message = "All ECR repositories must be immutable."
  }

  assert {
    condition     = alltrue([for repository in aws_ecr_repository.this : repository.image_scanning_configuration[0].scan_on_push])
    error_message = "All ECR repositories must scan on push."
  }
}

run "oidc_cross_account_promotion" {
  command = plan

  variables {
    name_prefix = "test-staging"
    kms_key_arn = "arn:aws:kms:sa-east-1:222222222222:key/ecr"
    repositories = {
      api = { name = "test/api" }
    }
    promotion_oidc_provider_arn = "arn:aws:iam::222222222222:oidc-provider/token.actions.githubusercontent.com"
    promotion_oidc_subjects = [
      "repo:example/ambient-scribes:environment:staging-promotion",
    ]
    promotion_source_repository_arns = [
      "arn:aws:ecr:sa-east-1:111111111111:repository/test/api",
    ]
  }

  assert {
    condition     = length(aws_iam_role.promotion) == 1
    error_message = "OIDC trust must create the destination promotion role."
  }
}
