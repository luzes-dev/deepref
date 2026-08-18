# EKS Pod Identity module

Creates least-privilege IAM roles, optional inline/managed policy attachments, and EKS Pod Identity associations. Role trust is limited to `pods.eks.amazonaws.com` with `sts:AssumeRole` and `sts:TagSession`.

This module intentionally does **not** use a Kubernetes provider and never creates or annotates a Kubernetes `ServiceAccount`. GitOps/Helm owns ServiceAccounts; its namespace and name must exactly match the association inputs. Do not add IRSA annotations to those accounts.

Apply-time prerequisites are an existing EKS cluster, the EKS Pod Identity Agent add-on, pre-reviewed policies, and AWS credentials allowed to create/pass IAM roles and create associations. Associations can be applied before the ServiceAccounts, but credentials are unavailable until both sides exist. Prefer small workload-specific inline policies over broad AWS-managed policies.
