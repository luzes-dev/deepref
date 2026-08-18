variable "account_id" {
  description = "AWS account that owns the keys."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.account_id))
    error_message = "account_id must be a 12-digit AWS account ID."
  }
}

variable "keys" {
  description = "Customer-managed keys keyed by a stable logical name."
  type = map(object({
    alias                = string
    description          = string
    service_principals   = optional(set(string), [])
    user_principal_arns  = optional(set(string), [])
    deletion_window_days = optional(number, 30)
    multi_region         = optional(bool, false)
  }))

  validation {
    condition = alltrue([
      for key in values(var.keys) : startswith(key.alias, "alias/")
    ])
    error_message = "Every key alias must start with alias/."
  }

  validation {
    condition = alltrue([
      for key in values(var.keys) : key.deletion_window_days >= 7 && key.deletion_window_days <= 30
    ])
    error_message = "deletion_window_days must be between 7 and 30."
  }

  validation {
    condition     = length(distinct([for key in values(var.keys) : key.alias])) == length(var.keys)
    error_message = "Every KMS alias must be unique."
  }
}

variable "administrator_principal_arns" {
  description = "IAM principals allowed to administer keys in addition to the account root."
  type        = set(string)
  default     = []
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
