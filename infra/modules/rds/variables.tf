variable "name" {
  description = "RDS identifier and resource name prefix."
  type        = string
}

variable "deployment_tier" {
  description = "Safety contract applied to the database."
  type        = string

  validation {
    condition     = contains(["development", "staging", "production"], var.deployment_tier)
    error_message = "deployment_tier must be development, staging, or production."
  }
}

variable "vpc_id" {
  description = "VPC containing the database."
  type        = string
}

variable "vpc_cidr" {
  description = "VPC CIDR used to restrict database egress."
  type        = string
}

variable "subnet_ids" {
  description = "Exactly three isolated data subnet IDs."
  type        = list(string)

  validation {
    condition     = length(var.subnet_ids) == 3
    error_message = "subnet_ids must contain exactly three data subnets."
  }
}

variable "application_security_group_ids" {
  description = "Security groups allowed to connect to PostgreSQL."
  type        = set(string)
  default     = []
}

variable "allowed_cidr_blocks" {
  description = "Additional private CIDRs allowed to connect to PostgreSQL."
  type        = set(string)
  default     = []
}

variable "database_name" {
  description = "Initial PostgreSQL database name."
  type        = string
  default     = "ambient_scribes"
}

variable "master_username" {
  description = "Master username. AWS manages the password in Secrets Manager."
  type        = string
  default     = "dbadmin"
}

variable "instance_class" {
  description = "RDS instance class."
  type        = string
}

variable "allocated_storage_gib" {
  description = "Initial gp3 storage allocation in GiB."
  type        = number
}

variable "max_allocated_storage_gib" {
  description = "Maximum storage autoscaling allocation in GiB."
  type        = number
}

variable "multi_az" {
  description = "Whether RDS maintains a synchronous standby in another AZ."
  type        = bool
}

variable "deletion_protection" {
  description = "Whether RDS API deletion protection is enabled."
  type        = bool
}

variable "backup_retention_days" {
  description = "Automated backup/PITR retention in days."
  type        = number

  validation {
    condition     = var.backup_retention_days >= 1 && var.backup_retention_days <= 35
    error_message = "backup_retention_days must be between 1 and 35."
  }
}

variable "preferred_availability_zone" {
  description = "Optional preferred AZ for Single-AZ deployments."
  type        = string
  default     = null
}

variable "kms_key_arn" {
  description = "KMS key ARN used for database storage, snapshots, and Performance Insights."
  type        = string
}

variable "master_secret_kms_key_arn" {
  description = "KMS key ARN used by the AWS-managed master password secret."
  type        = string
}

variable "backup_window" {
  description = "UTC automated backup window."
  type        = string
  default     = "03:00-04:00"
}

variable "maintenance_window" {
  description = "UTC weekly maintenance window."
  type        = string
  default     = "sun:05:00-sun:06:00"
}

variable "performance_insights_retention_days" {
  description = "Performance Insights retention period."
  type        = number
  default     = 7

  validation {
    condition     = contains([7, 731], var.performance_insights_retention_days)
    error_message = "Performance Insights retention must be 7 or 731 days."
  }
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
