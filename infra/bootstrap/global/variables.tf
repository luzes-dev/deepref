variable "expected_account_id" {
  description = "AWS account explicitly selected to anchor global state; execution fails in every other account."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.expected_account_id))
    error_message = "expected_account_id must be a 12-digit AWS account ID."
  }
}

variable "aws_region" {
  description = "AWS region containing the global state backend."
  type        = string
  default     = "sa-east-1"

  validation {
    condition     = var.aws_region == "sa-east-1"
    error_message = "The global state backend must remain in sa-east-1."
  }
}

variable "project_name" {
  description = "Stable project slug."
  type        = string
  default     = "ambient-scribes"
}

variable "state_bucket_name" {
  description = "Globally unique global state bucket name."
  type        = string
}

variable "state_kms_alias" {
  description = "Dedicated global state KMS alias."
  type        = string
  default     = "alias/ambient-scribes-global-state"
}

variable "state_access_principal_arns" {
  description = "Existing operator and CI roles allowed to access global state and native locks."
  type        = set(string)
}

variable "kms_administrator_principal_arns" {
  description = "Existing roles allowed to administer the global state KMS key."
  type        = set(string)
  default     = []
}

variable "noncurrent_version_retention_days" {
  description = "Days to retain previous global state and lock object versions."
  type        = number
  default     = 365
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
