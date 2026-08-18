provider "aws" {
  region = var.aws_region

  default_tags {
    tags = merge(var.tags, {
      Environment = "production"
      ManagedBy   = "OpenTofu"
      Project     = var.project_name
    })
  }
}

data "aws_caller_identity" "current" {}
