variable "name" {
  description = "Stable environment-qualified observability name."
  type        = string
}

variable "amp_kms_key_arn" {
  description = "Customer-managed KMS key ARN for Amazon Managed Prometheus."
  type        = string
}

variable "logs_kms_key_arn" {
  description = "Customer-managed KMS key ARN for CloudWatch log groups."
  type        = string
}

variable "grafana_kms_key_arn" {
  description = "Customer-managed KMS key ARN for Amazon Managed Grafana workspace data."
  type        = string
}

variable "xray_kms_key_arn" {
  description = "Customer-managed KMS key ARN for regional X-Ray trace encryption."
  type        = string
}

variable "log_groups" {
  description = "CloudWatch log groups keyed by logical name. Include an amp key for AMP logging."
  type = map(object({
    name              = string
    retention_in_days = number
  }))

  validation {
    condition     = contains(keys(var.log_groups), "amp")
    error_message = "log_groups must contain an amp entry for Managed Prometheus logging."
  }

  validation {
    condition = alltrue([
      for log_group in values(var.log_groups) : contains([
        1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653,
      ], log_group.retention_in_days)
    ])
    error_message = "Every log retention value must be supported by CloudWatch Logs."
  }
}

variable "grafana_admin_user_ids" {
  description = "Optional IAM Identity Center user IDs assigned the Grafana ADMIN role."
  type        = set(string)
  default     = []
}

variable "grafana_editor_user_ids" {
  description = "Optional IAM Identity Center user IDs assigned the Grafana EDITOR role."
  type        = set(string)
  default     = []
}

variable "grafana_viewer_user_ids" {
  description = "Optional IAM Identity Center user IDs assigned the Grafana VIEWER role."
  type        = set(string)
  default     = []
}

variable "xray_sampling_rules" {
  description = "X-Ray sampling rules keyed by stable logical name."
  type = map(object({
    priority       = number
    reservoir_size = optional(number, 1)
    fixed_rate     = optional(number, 0.05)
    host           = optional(string, "*")
    http_method    = optional(string, "*")
    url_path       = optional(string, "*")
    service_name   = optional(string, "*")
    service_type   = optional(string, "*")
    resource_arn   = optional(string, "*")
    attributes     = optional(map(string), {})
  }))
  default = {
    platform = {
      priority       = 10000
      reservoir_size = 1
      fixed_rate     = 0.05
    }
  }

  validation {
    condition = alltrue([
      for rule in values(var.xray_sampling_rules) : rule.fixed_rate >= 0 && rule.fixed_rate <= 1 && rule.reservoir_size >= 0
    ])
    error_message = "X-Ray fixed_rate must be 0-1 and reservoir_size must be non-negative."
  }
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
