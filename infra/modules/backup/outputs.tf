output "vault_arn" {
  description = "Backup vault ARN."
  value       = aws_backup_vault.this.arn
}

output "vault_name" {
  description = "Backup vault name."
  value       = aws_backup_vault.this.name
}

output "plan_id" {
  description = "Backup plan ID."
  value       = aws_backup_plan.this.id
}

output "selection_role_arn" {
  description = "Role used by AWS Backup for backup and restore operations."
  value       = local.effective_role_arn
}

output "continuous_backup_enabled" {
  description = "Whether supported selected resources receive continuous/PITR recovery points."
  value       = var.enable_continuous_backup
}
