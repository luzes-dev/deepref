locals {
  common_tags         = merge(var.tags, { ManagedBy = "OpenTofu" })
  create_service_role = var.selection_role_arn == null
  effective_role_arn  = local.create_service_role ? aws_iam_role.backup[0].arn : var.selection_role_arn
}

resource "aws_backup_vault" "this" {
  name        = var.name
  kms_key_arn = var.kms_key_arn
  tags        = local.common_tags
}

resource "aws_backup_vault_lock_configuration" "this" {
  backup_vault_name   = aws_backup_vault.this.name
  min_retention_days  = var.vault_lock_min_retention_days
  max_retention_days  = var.vault_lock_max_retention_days
  changeable_for_days = var.vault_lock_changeable_for_days

  lifecycle {
    precondition {
      condition     = var.vault_lock_min_retention_days >= 1 && var.vault_lock_max_retention_days >= var.vault_lock_min_retention_days
      error_message = "Vault Lock maximum retention must be at least the positive minimum retention."
    }

    precondition {
      condition     = var.delete_after_days >= var.vault_lock_min_retention_days && var.delete_after_days <= var.vault_lock_max_retention_days
      error_message = "The plan retention must fit within the Vault Lock minimum and maximum."
    }
  }
}

resource "aws_iam_role" "backup" {
  count = local.create_service_role ? 1 : 0

  name = "${var.name}-backup"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "backup.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "backup" {
  count = local.create_service_role ? 1 : 0

  role       = aws_iam_role.backup[0].name
  policy_arn = "arn:${data.aws_partition.current.partition}:iam::aws:policy/service-role/AWSBackupServiceRolePolicyForBackup"
}

resource "aws_iam_role_policy_attachment" "restore" {
  count = local.create_service_role ? 1 : 0

  role       = aws_iam_role.backup[0].name
  policy_arn = "arn:${data.aws_partition.current.partition}:iam::aws:policy/service-role/AWSBackupServiceRolePolicyForRestores"
}

data "aws_partition" "current" {}

resource "aws_backup_plan" "this" {
  name = var.name

  rule {
    rule_name                = "${var.name}-protected"
    target_vault_name        = aws_backup_vault.this.name
    schedule                 = var.schedule
    start_window             = var.start_window_minutes
    completion_window        = var.completion_window_minutes
    enable_continuous_backup = var.enable_continuous_backup
    recovery_point_tags      = merge(local.common_tags, var.recovery_point_tags)

    lifecycle {
      cold_storage_after = var.cold_storage_after_days
      delete_after       = var.delete_after_days
    }
  }

  tags = local.common_tags

  lifecycle {
    precondition {
      condition     = !var.enable_continuous_backup || var.delete_after_days <= 35
      error_message = "AWS Backup continuous/PITR recovery points cannot retain more than 35 days."
    }

    precondition {
      condition     = var.cold_storage_after_days == null || var.delete_after_days >= var.cold_storage_after_days + 90
      error_message = "Cold recovery points must remain retained for at least 90 days after transition."
    }
  }

  depends_on = [aws_backup_vault_lock_configuration.this]
}

resource "aws_backup_selection" "this" {
  name         = "${var.name}-resources"
  iam_role_arn = local.effective_role_arn
  plan_id      = aws_backup_plan.this.id
  resources    = sort(tolist(var.resource_arns))

  depends_on = [
    aws_iam_role_policy_attachment.backup,
    aws_iam_role_policy_attachment.restore,
  ]
}
