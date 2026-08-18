variable "secrets" {
  description = "Secret containers keyed by a stable logical name. Values are intentionally not accepted."
  type = map(object({
    name                  = string
    description           = string
    reader_principal_arns = optional(set(string), [])
    recovery_window_days  = optional(number, 30)
    replica_regions       = optional(set(string), [])
  }))

  validation {
    condition = alltrue([
      for secret in values(var.secrets) : secret.recovery_window_days >= 7 && secret.recovery_window_days <= 30
    ])
    error_message = "recovery_window_days must be between 7 and 30."
  }

  validation {
    condition     = length(distinct([for secret in values(var.secrets) : secret.name])) == length(var.secrets)
    error_message = "Every secret name must be unique."
  }
}

variable "kms_key_arn" {
  description = "Customer-managed KMS key ARN for all primary secret containers."
  type        = string
}

variable "replica_kms_key_arns" {
  description = "Optional KMS key ARNs keyed by replica region."
  type        = map(string)
  default     = {}
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
