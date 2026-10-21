# Business gate decisions

Recorded 2026-10-19 by the product owner. This resolves part of the commercial readiness gate in `00_PROGRAM.md` and `08_POLAR_BILLING_ENTITLEMENTS.md`. It is not legal advice; have a lawyer or contador review the published privacy policy before launch.

## Seller and payouts

```text
seller           an individual resident in Brazil; no company (CNPJ) or foreign entity today
Polar            not eligible: Polar pays out through Stripe Connect Express only to the countries
                 on https://polar.sh/docs/merchant-of-record/supported-countries, and Brazil is not
                 listed (checked 2026-10-19)
billing provider undecided
```

Candidates, to be decided before 08 starts:

```text
Paddle                    Merchant of Record; lists Brazil as a supported country, takes Pix and Pix
                          Automático. Confirm in writing that Paddle pays out to a Brazilian individual.
Polar via foreign entity  keep Polar; open a company in a supported country
Stripe Brasil / local PSP no Merchant of Record; DeepRef collects tax and issues NFS-e; needs a CNPJ
```

Consequences:

- 08 is paused. It is written for Polar and is rewritten for the chosen provider before any billing code.
- 09 proceeds. Its downstream usage export goes through a provider-neutral port: the local ledger, settlement and outbox record are unchanged, and the provider adapter is chosen with 08. Read "Polar" in 09 as "the billing provider" until then.

## Data region

Production PostgreSQL, its backups and the object-storage bucket run in a São Paulo region (for example AWS `sa-east-1`, GCP `southamerica-east1` or Azure Brazil South). Hosting provider choice stays with ADR 0008. WorkOS, the billing provider and the AI providers process data outside Brazil. Those international transfers rely on the ANPD standard contractual clauses (Resolution CD/ANPD 19/2024) or the provider's equivalent.

## LGPD roles and requests

```text
controller          the product owner, as an individual
encarregado (DPO)   not appointed: small processing agents are exempt under Resolution CD/ANPD 2/2022,
                    provided a contact channel exists. Re-check if processing becomes high-risk
                    (large scale combined with emerging technology such as AI, automated decisions
                    or sensitive data).
contact channel     a published privacy email address, also used by the ANPD
response deadline   access requests answered within 15 days (LGPD art. 19)
processors          WorkOS, the billing provider, AI providers, hosting and object storage; listed in
                    the privacy policy
```

Self-service, in the product:

```text
Download my data    ZIP of JSON plus uploaded files, covering account, workspaces the user owns,
                    and their billing projection
Delete my account   applies the deletion periods below; the email channel covers anything else
```

## Retention

| Data                                                 | Retention                                                                    | Reason                                                                          |
| ---------------------------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| Account (name, email, WorkOS identity)               | While active; deleted 30 days after a deletion request                       | Service delivery                                                                |
| Research content in a workspace the user solely owns | Deleted 30 days after the owner deletes it or the account                    | Service delivery                                                                |
| Contributions in shared workspaces                   | Kept under an anonymized "former member" label                               | Co-authors' review integrity; must stay consistent with 03c identity continuity |
| Sessions                                             | 30 days after expiry or revocation                                           | Security                                                                        |
| Access and security logs, administrative audit       | 6 months                                                                     | Security; mirrors Marco Civil art. 15                                           |
| Record that a deletion request was fulfilled         | 5 years, no content                                                          | Proof of compliance                                                             |
| Billing and AI usage records                         | 5 years, linked to the billing account rather than the person after deletion | Brazilian tax limitation period                                                 |
| Integration inbox payloads                           | 30 days; dedup identities kept                                               | Implemented in 04a                                                              |
| Backups                                              | 35-day rolling PITR window                                                   | Deleted data leaves backups as they rotate                                      |
| Inactive free accounts                               | Warning at 22 months; deleted at 24 months                                   | Minimization                                                                    |

Restores re-apply a log of fulfilled deletions, so a restored backup never brings deleted personal data back. The privacy policy states: "deleted within 30 days, and removed from backups within a further 35 days."
