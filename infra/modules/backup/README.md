# Backup module

Creates a KMS-encrypted AWS Backup vault, Vault Lock configuration, backup plan, and explicit resource selection. By default it creates the AWS Backup service role and enables continuous recovery points for services that support PITR, with the AWS maximum 35-day continuous retention.

This complements, rather than replaces, native database PITR settings. Keep RDS automated backups enabled and test both RDS point-in-time restore and AWS Backup restore paths. Resources without continuous-backup support still receive scheduled snapshots.

`vault_lock_changeable_for_days = null` uses governance mode. A non-null value selects compliance mode and becomes immutable after its grace period: even the account root cannot shorten retention or remove the lock. Confirm retention, KMS access, legal requirements, and restore drills before applying compliance mode. Cold-storage recovery points must remain for at least 90 days after transition.

Apply-time prerequisites are an existing customer-managed KMS key whose policy permits AWS Backup, explicit resource ARNs, and either permission to create/pass the generated service role or a pre-existing least-privilege `selection_role_arn`.
