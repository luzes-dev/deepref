# DeepRef product requirements

This directory replaces the single DeepRef v2 plan with smaller product
requirements documents. Each document has one product boundary, explicit
invariants, acceptance checks, and links to implementation evidence.

The documents are ordered by dependency:

1. [Evidence identity and acquisition](01-evidence-identity-and-acquisition.md)
2. [Review protocol and screening](02-review-protocol-and-screening.md)
3. [Documents, studies, and appraisal](03-documents-studies-and-appraisal.md)
4. [PRISMA and graph projections](04-prisma-and-graph.md)
5. [AI proposals and grounding](05-ai-proposals-and-grounding.md)
6. [Automation and assistant](06-automation-and-assistant.md)
7. [Evaluation, audit, and operations](07-evaluation-audit-and-operations.md)

## How to audit a requirement

An item is complete only when its named code path and acceptance test agree.
Passing a type check or finding a route is not enough for a stateful
requirement. Reviewers should run the command in the relevant document and
inspect the linked test or migration.

The original technical plan remains useful as historical context. These PRDs
are the current product contract.
