mock_provider "aws" {}

run "metadata_only_secret_containers" {
  command = plan

  variables {
    kms_key_arn = "arn:aws:kms:sa-east-1:111111111111:key/00000000-0000-0000-0000-000000000000"
    secrets = {
      api = {
        name                  = "test/api"
        description           = "API configuration"
        reader_principal_arns = ["arn:aws:iam::111111111111:role/test-api"]
      }
      github = {
        name        = "test/github"
        description = "GitHub application configuration"
      }
    }
  }

  assert {
    condition     = length(aws_secretsmanager_secret.this) == 2
    error_message = "Every declared secret container must be created."
  }

  assert {
    condition     = length(aws_secretsmanager_secret_policy.reader) == 1
    error_message = "Only containers with readers should receive a resource policy."
  }
}
