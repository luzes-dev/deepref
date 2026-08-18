# Budgets and alerts module

Creates one customer-KMS-encrypted SNS operations topic, a monthly cost budget with actual/forecast thresholds, optional email subscriptions, and optional CloudWatch metric alarms. Topic policy grants publish only to AWS Budgets and CloudWatch from the expected account and retains account-owner administration.

The supplied KMS key policy must allow SNS to use the key (normally constrained with `kms:ViaService = sns.<region>.amazonaws.com`) and must support the AWS Budgets and CloudWatch delivery path. An AWS Budget does not prevent spend; it only notifies.

Email delivery is not operational until every recipient opens the AWS confirmation message. Treat unconfirmed subscriptions as an apply-time operational blocker and test the topic after confirmation. Alarm definitions should use dimensions that uniquely identify the intended resource and an explicit missing-data policy appropriate to the signal.
