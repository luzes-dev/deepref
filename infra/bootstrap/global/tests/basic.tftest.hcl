mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "111111111111"
    }
  }
}

run "guarded_native_lock_global_backend" {
  command = plan

  variables {
    expected_account_id = "111111111111"
    state_bucket_name   = "ambient-scribes-global-test-111111111111"
    state_access_principal_arns = [
      "arn:aws:iam::111111111111:role/ambient-scribes-global-infra",
    ]
  }

  assert {
    condition     = terraform_data.account_and_workspace_guard.input == "111111111111"
    error_message = "The bootstrap must bind its guard to the caller account."
  }

  assert {
    condition     = output.backend_configuration.use_lockfile
    error_message = "The global backend must use native S3 lockfiles."
  }
}
