# Business gate decisions

Recorded 2026-10-19 by the product owner. This resolves part of the commercial readiness gate in `00_PROGRAM.md` and `08_POLAR_BILLING_ENTITLEMENTS.md`. It is not legal advice; have a lawyer or contador review the published privacy policy before launch.

## Seller and payouts

```text
seller           an individual resident in Brazil; no company (CNPJ) or foreign entity today
Polar            not eligible: Polar pays out through Stripe Connect Express only to the countries
                 on https://polar.sh/docs/merchant-of-record/supported-countries, and Brazil is not
                 listed (checked 2026-10-19)
billing provider Dodo Payments selected 2026-10-23 (see update below)
```

Decision update, 2026-10-23: the owner selected **Dodo Payments** (“lets use dodo payments”). The seller remains an individual resident in Brazil, without a CNPJ or foreign entity. This replaces the previous provider-undecided decision and the original Polar-specific implementation target.

Dodo’s [merchant-country policy](https://docs.dodopayments.com/miscellaneous/accepted-countries-and-territories) lists Brazil. Individual eligibility depends on the issuing country of the merchant’s government ID, rather than residence alone. Its [verification process](https://docs.dodopayments.com/miscellaneous/verification-process) supports an Individual account without company registration, with product, identity and bank verification. Live payments and payouts have distinct approval requirements; payouts also require compliance review. The selection records an implementation target, not approval of this seller or product.

Consequences:

- Rewrite handoff 08 for Dodo Payments before substantial billing code. Keep its legacy filename to preserve existing links. Implement through a DeepRef-owned Rust provider port, with local entitlement projections and the existing PostgreSQL integration inbox.
- 09’s ledger, reservations, settlement and immutable usage identities remain authoritative. Add a Dodo adapter behind the existing provider-neutral export port. Its [usage-ingestion guide](https://docs.dodopayments.com/developer-resources/usage-based-billing-guide) rejects timestamps older than one hour. Late facts must remain visible for explicit reconciliation; never change a fact’s timestamp or identity to force acceptance in a different billing period.
- Test-mode implementation and recorded contract tests may proceed. Live activation requires the actual account/product/KYC/bank and payout approvals, configured commercial catalog, protected credentials and the published policies required by Dodo. No foreign company is created by this work.

## Full-target scope

Owner decision, 2026-10-23: “Replace gates; implement full target now.” NATS JetStream (04b/05b), Valkey (06), selective CQRS (10) and API keys/service principals (11A) are now required implementation work. The old measurement/product-need gates no longer defer them. Rollout compatibility, tenant isolation, conservative outage behavior and operational acceptance remain mandatory.

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
