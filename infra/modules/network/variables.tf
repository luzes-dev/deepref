variable "name" {
  description = "Name prefix used for network resources."
  type        = string

  validation {
    condition     = length(trimspace(var.name)) > 0
    error_message = "name must not be empty."
  }
}

variable "vpc_cidr" {
  description = "IPv4 CIDR assigned to the VPC."
  type        = string
}

variable "availability_zones" {
  description = "Exactly three availability zones used by all subnet tiers."
  type        = list(string)

  validation {
    condition     = length(var.availability_zones) == 3 && length(distinct(var.availability_zones)) == 3
    error_message = "availability_zones must contain exactly three distinct zones."
  }
}

variable "public_subnet_cidrs" {
  description = "Public subnet CIDRs, ordered to match availability_zones."
  type        = list(string)

  validation {
    condition     = length(var.public_subnet_cidrs) == 3
    error_message = "public_subnet_cidrs must contain exactly three CIDRs."
  }
}

variable "private_subnet_cidrs" {
  description = "Private workload subnet CIDRs, ordered to match availability_zones."
  type        = list(string)

  validation {
    condition     = length(var.private_subnet_cidrs) == 3
    error_message = "private_subnet_cidrs must contain exactly three CIDRs."
  }
}

variable "data_subnet_cidrs" {
  description = "Isolated data subnet CIDRs, ordered to match availability_zones."
  type        = list(string)

  validation {
    condition     = length(var.data_subnet_cidrs) == 3
    error_message = "data_subnet_cidrs must contain exactly three CIDRs."
  }
}

variable "nat_gateway_mode" {
  description = "Use one shared NAT gateway or one NAT gateway per availability zone."
  type        = string

  validation {
    condition     = contains(["single", "one_per_az"], var.nat_gateway_mode)
    error_message = "nat_gateway_mode must be single or one_per_az."
  }
}

variable "interface_endpoint_services" {
  description = "Regional AWS interface endpoint service suffixes."
  type        = set(string)
  default = [
    "ecr.api",
    "ecr.dkr",
    "ec2messages",
    "logs",
    "monitoring",
    "secretsmanager",
    "ssm",
    "ssmmessages",
    "sts",
  ]
}

variable "flow_log_kms_key_arn" {
  description = "Optional KMS key ARN used to encrypt the VPC flow log group."
  type        = string
  default     = null
}

variable "flow_log_retention_days" {
  description = "CloudWatch retention period for VPC flow logs."
  type        = number
  default     = 90
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
