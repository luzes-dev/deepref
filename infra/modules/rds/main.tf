locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
}

resource "aws_db_subnet_group" "this" {
  name       = var.name
  subnet_ids = var.subnet_ids
  tags       = merge(local.common_tags, { Name = var.name })
}

resource "aws_security_group" "this" {
  name_prefix = "${var.name}-db-"
  description = "PostgreSQL ingress from approved workloads"
  vpc_id      = var.vpc_id

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = [var.vpc_cidr]
  }

  tags = merge(local.common_tags, { Name = "${var.name}-db" })
}

resource "aws_vpc_security_group_ingress_rule" "application" {
  for_each = var.application_security_group_ids

  security_group_id            = aws_security_group.this.id
  referenced_security_group_id = each.value
  description                  = "PostgreSQL from application security group"
  ip_protocol                  = "tcp"
  from_port                    = 5432
  to_port                      = 5432
}

resource "aws_vpc_security_group_ingress_rule" "cidr" {
  for_each = var.allowed_cidr_blocks

  security_group_id = aws_security_group.this.id
  cidr_ipv4         = each.value
  description       = "PostgreSQL from approved private CIDR"
  ip_protocol       = "tcp"
  from_port         = 5432
  to_port           = 5432
}

resource "aws_db_parameter_group" "this" {
  name_prefix = "${var.name}-pg17-"
  family      = "postgres17"
  description = "Audited PostgreSQL 17 defaults for ${var.name}"

  parameter {
    name  = "rds.force_ssl"
    value = "1"
  }

  parameter {
    name  = "log_connections"
    value = "1"
  }

  parameter {
    name  = "log_disconnections"
    value = "1"
  }

  parameter {
    name  = "log_lock_waits"
    value = "1"
  }

  tags = local.common_tags

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_db_instance" "this" {
  identifier = var.name

  engine         = "postgres"
  engine_version = "17"
  instance_class = var.instance_class

  db_name  = var.database_name
  username = var.master_username
  port     = 5432

  manage_master_user_password   = true
  master_user_secret_kms_key_id = var.master_secret_kms_key_arn

  allocated_storage     = var.allocated_storage_gib
  max_allocated_storage = var.max_allocated_storage_gib
  storage_type          = "gp3"
  storage_encrypted     = true
  kms_key_id            = var.kms_key_arn

  multi_az               = var.multi_az
  availability_zone      = var.multi_az ? null : var.preferred_availability_zone
  db_subnet_group_name   = aws_db_subnet_group.this.name
  vpc_security_group_ids = [aws_security_group.this.id]
  publicly_accessible    = false
  network_type           = "IPV4"

  parameter_group_name       = aws_db_parameter_group.this.name
  auto_minor_version_upgrade = true
  apply_immediately          = false

  backup_retention_period = var.backup_retention_days
  backup_window           = var.backup_window
  maintenance_window      = var.maintenance_window
  copy_tags_to_snapshot   = true

  deletion_protection       = var.deletion_protection
  skip_final_snapshot       = !var.deletion_protection
  final_snapshot_identifier = "${var.name}-final"

  enabled_cloudwatch_logs_exports       = ["postgresql", "upgrade"]
  iam_database_authentication_enabled   = true
  performance_insights_enabled          = true
  performance_insights_kms_key_id       = var.kms_key_arn
  performance_insights_retention_period = var.performance_insights_retention_days

  tags = merge(local.common_tags, { Name = var.name })

  lifecycle {
    precondition {
      condition     = var.max_allocated_storage_gib >= var.allocated_storage_gib
      error_message = "max_allocated_storage_gib must be at least allocated_storage_gib."
    }

    precondition {
      condition = var.deployment_tier != "production" || (
        var.multi_az && var.deletion_protection && var.backup_retention_days == 35
      )
      error_message = "Production requires Multi-AZ, deletion protection, and exactly 35 days of PITR retention."
    }

    precondition {
      condition = var.deployment_tier == "production" || (
        !var.multi_az && !var.deletion_protection
      )
      error_message = "Development and staging must remain Single-AZ without deletion protection."
    }
  }
}
