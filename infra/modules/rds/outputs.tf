output "instance_id" {
  description = "RDS instance identifier."
  value       = aws_db_instance.this.id
}

output "instance_arn" {
  description = "RDS instance ARN."
  value       = aws_db_instance.this.arn
}

output "endpoint" {
  description = "PostgreSQL endpoint without credentials."
  value       = aws_db_instance.this.endpoint
}

output "address" {
  description = "PostgreSQL DNS address."
  value       = aws_db_instance.this.address
}

output "port" {
  description = "PostgreSQL port."
  value       = aws_db_instance.this.port
}

output "security_group_id" {
  description = "Database security group identifier."
  value       = aws_security_group.this.id
}

output "master_user_secret_arn" {
  description = "AWS-managed Secrets Manager ARN containing the master credentials."
  value       = try(aws_db_instance.this.master_user_secret[0].secret_arn, null)
  sensitive   = true
}
