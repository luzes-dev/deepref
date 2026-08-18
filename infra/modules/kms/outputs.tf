output "key_arns" {
  description = "KMS key ARNs keyed by logical name."
  value       = { for name, key in aws_kms_key.this : name => key.arn }
}

output "key_ids" {
  description = "KMS key IDs keyed by logical name."
  value       = { for name, key in aws_kms_key.this : name => key.key_id }
}

output "aliases" {
  description = "KMS aliases keyed by logical name."
  value       = { for name, alias in aws_kms_alias.this : name => alias.name }
}
