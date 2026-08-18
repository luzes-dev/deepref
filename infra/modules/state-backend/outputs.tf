output "bucket_name" {
  description = "State bucket name."
  value       = aws_s3_bucket.state.id
}

output "bucket_arn" {
  description = "State bucket ARN."
  value       = aws_s3_bucket.state.arn
}

output "kms_key_arn" {
  description = "State KMS key ARN."
  value       = aws_kms_key.state.arn
}

output "kms_alias" {
  description = "State KMS alias."
  value       = aws_kms_alias.state.name
}

output "backend_configuration" {
  description = "Non-secret values required by an S3 backend configuration."
  value = {
    bucket       = aws_s3_bucket.state.id
    encrypt      = true
    kms_key_id   = aws_kms_key.state.arn
    use_lockfile = true
  }
}
