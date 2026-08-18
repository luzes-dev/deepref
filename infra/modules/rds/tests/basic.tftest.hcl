mock_provider "aws" {}

run "production_durability" {
  command = plan

  variables {
    name                      = "test-production"
    deployment_tier           = "production"
    vpc_id                    = "vpc-00000000"
    subnet_ids                = ["subnet-a", "subnet-b", "subnet-c"]
    instance_class            = "db.r7g.large"
    allocated_storage_gib     = 100
    max_allocated_storage_gib = 500
    multi_az                  = true
    deletion_protection       = true
    backup_retention_days     = 35
    kms_key_arn               = "arn:aws:kms:sa-east-1:111111111111:key/rds"
    master_secret_kms_key_arn = "arn:aws:kms:sa-east-1:111111111111:key/secrets"
  }

  assert {
    condition     = aws_db_instance.this.multi_az && aws_db_instance.this.deletion_protection
    error_message = "Production must be Multi-AZ and deletion protected."
  }

  assert {
    condition     = aws_db_instance.this.backup_retention_period == 35
    error_message = "Production must retain 35 days of PITR history."
  }

  assert {
    condition     = aws_db_instance.this.storage_encrypted && !aws_db_instance.this.publicly_accessible
    error_message = "The database must be encrypted and private."
  }
}

run "staging_single_az" {
  command = plan

  variables {
    name                        = "test-staging"
    deployment_tier             = "staging"
    vpc_id                      = "vpc-00000000"
    subnet_ids                  = ["subnet-a", "subnet-b", "subnet-c"]
    instance_class              = "db.r7g.large"
    allocated_storage_gib       = 50
    max_allocated_storage_gib   = 200
    multi_az                    = false
    deletion_protection         = false
    backup_retention_days       = 14
    preferred_availability_zone = "sa-east-1a"
    kms_key_arn                 = "arn:aws:kms:sa-east-1:222222222222:key/rds"
    master_secret_kms_key_arn   = "arn:aws:kms:sa-east-1:222222222222:key/secrets"
  }

  assert {
    condition     = !aws_db_instance.this.multi_az
    error_message = "Staging must remain Single-AZ."
  }
}
