output "state_bucket_name" {
  description = "Production OpenTofu state bucket name."
  value       = module.state_backend.bucket_name
}

output "state_bucket_arn" {
  description = "Production OpenTofu state bucket ARN."
  value       = module.state_backend.bucket_arn
}

output "state_kms_key_arn" {
  description = "Production state KMS key ARN."
  value       = module.state_backend.kms_key_arn
}

output "backend_configuration" {
  description = "Base remote-backend settings; add the root-specific key from backend.hcl.example."
  value       = module.state_backend.backend_configuration
}
