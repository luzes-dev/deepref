variable "account_id" {
  description = "Twelve-digit AWS account ID that owns the state backend."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.account_id))
    error_message = "account_id must be a 12-digit AWS account ID."
  }
}

variable "bucket_name" {
  description = "Globally unique S3 bucket name for OpenTofu state and native lock objects."
  type        = string

  validation {
    condition     = length(var.bucket_name) >= 3 && length(var.bucket_name) <= 63 && can(regex("^[a-z0-9][a-z0-9.-]*[a-z0-9]$", var.bucket_name))
    error_message = "bucket_name must be a valid 3-63 character S3 bucket name."
  }
}

variable "kms_alias" {
  description = "KMS alias dedicated to state encryption, including the alias/ prefix."
  type        = string

  validation {
    condition     = can(regex("^alias/[A-Za-z0-9/_-]+$", var.kms_alias))
    error_message = "kms_alias must start with alias/."
  }
}

variable "kms_administrator_principal_arns" {
  description = "IAM principals allowed to administer the state KMS key."
  type        = set(string)
  default     = []
}

variable "state_access_principal_arns" {
  description = "IAM principals allowed to read/write state and native lock objects and use the KMS key."
  type        = set(string)

  validation {
    condition     = length(var.state_access_principal_arns) > 0
    error_message = "At least one state access principal is required to avoid an unusable backend."
  }
}

variable "noncurrent_version_retention_days" {
  description = "Days to retain noncurrent state and lock object versions."
  type        = number
  default     = 365

  validation {
    condition     = var.noncurrent_version_retention_days >= 30
    error_message = "Noncurrent state versions must be retained for at least 30 days."
  }
}

variable "kms_deletion_window_days" {
  description = "KMS deletion waiting period."
  type        = number
  default     = 30

  validation {
    condition     = var.kms_deletion_window_days >= 7 && var.kms_deletion_window_days <= 30
    error_message = "kms_deletion_window_days must be between 7 and 30."
  }
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
