# ADR 0003: UUID evidence identity during DOI compatibility

## Decision

`Record` is a source-level hit, `Report` is the canonical publication/document, and
`Study` is the underlying investigation. Reports are identified by UUID; DOI, PMID,
PMCID, arXiv, ISBN, registry, and other values are report identifiers and may be
absent or multiple.

Migration 0007 renames the DOI-keyed citation table to `legacy_citations` and adds a
UUID-keyed `citations` table. Migration 0008 makes the UUID graph the runtime read
model and recomputes project metrics in PostgreSQL. The canonical cutover migrations
backfill DOI acquisition items and graph provenance before removing the legacy
runtime tables. The `import-legacy` command and `/ingestions` API are retired.

## Consequences

The UUID evidence model can preserve multiple source observations without making a
DOI the entity identity. DOI imports now use project-scoped acquisition creation,
items, refresh, and cancellation; stored run depth remains visible as provenance.
