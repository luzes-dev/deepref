variable "account_id" {
  description = "AWS account ID in which budgets and alarms are created."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.account_id))
    error_message = "account_id must be a 12-digit AWS account ID."
  }
}

variable "name" {
  description = "Name used for the SNS topic and monthly cost budget."
  type        = string
}

variable "sns_kms_key_arn" {
  description = "Customer-managed KMS key ARN for SNS message encryption."
  type        = string
}

variable "monthly_budget_amount" {
  description = "Monthly cost budget in the configured currency."
  type        = number

  validation {
    condition     = var.monthly_budget_amount > 0
    error_message = "monthly_budget_amount must be greater than zero."
  }
}

variable "currency" {
  description = "ISO-like budget currency unit accepted by AWS Budgets."
  type        = string
  default     = "USD"
}

variable "budget_notifications" {
  description = "Percentage thresholds and ACTUAL or FORECASTED notification type."
  type = map(object({
    threshold         = number
    notification_type = optional(string, "ACTUAL")
  }))
  default = {
    actual_80    = { threshold = 80 }
    actual_100   = { threshold = 100 }
    forecast_100 = { threshold = 100, notification_type = "FORECASTED" }
  }

  validation {
    condition = alltrue([
      for notification in values(var.budget_notifications) : notification.threshold > 0 && contains(["ACTUAL", "FORECASTED"], notification.notification_type)
    ])
    error_message = "Budget notifications require a positive threshold and ACTUAL or FORECASTED type."
  }
}

variable "email_subscribers" {
  description = "Operations email addresses that must confirm their SNS subscriptions."
  type        = set(string)
  default     = []

  validation {
    condition     = alltrue([for address in var.email_subscribers : can(regex("^[^@[:space:]]+@[^@[:space:]]+\\.[^@[:space:]]+$", address))])
    error_message = "Every email_subscriber must look like an email address."
  }
}

variable "metric_alarms" {
  description = "CloudWatch metric alarms that publish to the same encrypted operations topic."
  type = map(object({
    namespace                 = string
    metric_name               = string
    statistic                 = optional(string, "Average")
    period_seconds            = optional(number, 300)
    evaluation_periods        = optional(number, 2)
    datapoints_to_alarm       = optional(number)
    threshold                 = number
    comparison_operator       = string
    dimensions                = optional(map(string), {})
    treat_missing_data        = optional(string, "missing")
    alarm_description         = optional(string)
    insufficient_data_actions = optional(bool, false)
  }))
  default = {}
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
