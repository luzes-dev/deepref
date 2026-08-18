mock_provider "aws" {}

run "encrypted_versioned_native_lock_backend" {
  command = plan

  variables {
    account_id   = "111111111111"
    bucket_name  = "ambient-scribes-test-state-111111111111"
    kms_alias    = "alias/ambient-scribes-test-state"
    state_access_principal_arns = [
      "arn:aws:iam::111111111111:role/ambient-scribes-infra",
    ]
  }

  assert {
    condition     = aws_s3_bucket_versioning.state.versioning_configuration[0].status == "Enabled"
    error_message = "State bucket versioning must be enabled."
  }

  assert {
    condition     = aws_s3_bucket_server_side_encryption_configuration.state.rule[0].apply_server_side_encryption_by_default[0].sse_algorithm == "aws:kms"
    error_message = "State objects must use KMS encryption."
  }

  assert {
    condition     = aws_kms_key.state.enable_key_rotation
    error_message = "The state key must rotate."
  }

  assert {
    condition     = output.backend_configuration.use_lockfile
    error_message = "Backend output must select native S3 lockfiles."
  }
}
