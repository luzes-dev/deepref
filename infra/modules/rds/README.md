# RDS PostgreSQL module

Creates private, encrypted PostgreSQL 17 on isolated data subnets. AWS generates and rotates the master password in Secrets Manager; no password enters configuration or state as an input.

The module enforces environment durability contracts at plan time:

- `production`: Multi-AZ, deletion protection, and exactly 35 days of automated backup/PITR retention.
- `development` and `staging`: Single-AZ without deletion protection.

All tiers enforce TLS, export PostgreSQL and upgrade logs, enable IAM database authentication and encrypted Performance Insights, and accept ingress only from explicitly supplied security groups or CIDRs.
