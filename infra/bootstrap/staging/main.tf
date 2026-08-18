locals {
  environment = "staging"
  common_tags = merge(var.tags, {
    Environment = local.environment
    Project     = var.project_name
  })
}

resource "terraform_data" "account_and_workspace_guard" {
  input = data.aws_caller_identity.current.account_id

  lifecycle {
    precondition {
      condition     = data.aws_caller_identity.current.account_id == var.expected_account_id
      error_message = "Refusing to bootstrap staging from an unexpected AWS account."
    }

    precondition {
      condition     = terraform.workspace == "default"
      error_message = "Bootstrap roots are environment-specific; OpenTofu workspaces are forbidden."
    }
  }
}

module "state_backend" {
  source = "../../modules/state-backend"

  account_id                         = var.expected_account_id
  bucket_name                        = var.state_bucket_name
  kms_alias                          = var.state_kms_alias
  kms_administrator_principal_arns   = var.kms_administrator_principal_arns
  state_access_principal_arns        = var.state_access_principal_arns
  noncurrent_version_retention_days  = var.noncurrent_version_retention_days
  tags                               = local.common_tags

  depends_on = [terraform_data.account_and_workspace_guard]
}
