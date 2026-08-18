variable "name" {
  description = "Name used for the backup vault, plan, and service role."
  type        = string

  validation {
    condition     = length(trimspace(var.name)) > 0
    error_message = "name must not be empty."
  }
}

variable "kms_key_arn" {
  description = "Customer-managed KMS key ARN used by the backup vault."
  type        = string
}

variable "resource_arns" {
  description = "Explicit AWS resource ARNs protected by the plan."
  type        = set(string)

  validation {
    condition     = length(var.resource_arns) > 0
    error_message = "At least one backup resource ARN is required."
  }
}

variable "selection_role_arn" {
  description = "Existing AWS Backup selection role ARN. When null, the module creates a service role."
  type        = string
  default     = null
  nullable    = true
}

variable "schedule" {
  description = "AWS Backup cron expression for recovery-point snapshots."
  type        = string
  default     = "cron(0 5 ? * * *)"
}

variable "start_window_minutes" {
  description = "Minutes after the scheduled time in which a backup may start."
  type        = number
  default     = 60
}

variable "completion_window_minutes" {
  description = "Minutes after start in which a backup must complete."
  type        = number
  default     = 360
}

variable "enable_continuous_backup" {
  description = "Enable AWS Backup continuous recovery points for services that support PITR."
  type        = bool
  default     = true
}

variable "delete_after_days" {
  description = "Days after creation when recovery points expire. Continuous backups support at most 35 days."
  type        = number
  default     = 35

  validation {
    condition     = var.delete_after_days >= 1
    error_message = "delete_after_days must be positive."
  }
}

variable "cold_storage_after_days" {
  description = "Optional days before eligible snapshot recovery points transition to cold storage."
  type        = number
  default     = null
  nullable    = true
}

variable "vault_lock_min_retention_days" {
  description = "Minimum retention enforced by Backup Vault Lock."
  type        = number
  default     = 7
}

variable "vault_lock_max_retention_days" {
  description = "Maximum retention enforced by Backup Vault Lock."
  type        = number
  default     = 3650
}

variable "vault_lock_changeable_for_days" {
  description = "Compliance-mode grace period. Null selects governance mode; 3-36500 permanently locks after the grace period."
  type        = number
  default     = null
  nullable    = true

  validation {
    condition     = var.vault_lock_changeable_for_days == null || (var.vault_lock_changeable_for_days >= 3 && var.vault_lock_changeable_for_days <= 36500)
    error_message = "vault_lock_changeable_for_days must be null or between 3 and 36500."
  }
}

variable "recovery_point_tags" {
  description = "Additional tags copied to recovery points."
  type        = map(string)
  default     = {}
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
