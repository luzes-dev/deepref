# Migrations

SQLx applies these files in version order and records a SHA-384 checksum of
each file's full text in `_sqlx_migrations`.

**An applied migration file is immutable.** sqlx re-hashes the file on every
`Migrator::run` and fails with `MigrateError::VersionMismatch` if a single
byte changed — comments and whitespace included. Change the schema with a new
numbered migration; never edit an existing one.

- File name: `NNNN_snake_case_description.sql`, four-digit version prefix.
- One logical change per migration. Header comments describe the change
  itself, not the review or change series it was authored under.
- A migration that has shipped anywhere may only be rewritten after every
  database that applied it is dropped and recreated — including the
  `deepref_test_template` database used by
  `deepref_app::test_isolation`.

## DOI acquisition plane retirement

Migrations 0080–0081 are an offline expand/backfill/contract cutover. Stop API
producers and all workers before applying them, then deploy the canonical
acquisition API and worker together. Do not run a legacy worker against the new
schema. The migrations preserve report UUIDs through identifier mappings, run
IDs and historical depth, every DOI item, cache leases, unresolved references,
and existing job IDs/dedupe/queue leases. Unmapped pending event claims and
ambiguous DOI-alias citation provenance fail migration instead of losing work.

The unshipped 0070 migration preserves preexisting run depths above three with
private nullable `historical_max_depth` provenance. New ordinary inserts and
public DOI inputs remain bounded to 0–3; active settings/project defaults are
strictly bounded. Never populate that marker from user input.

`acquisition_runs` / `acquisition_items` own DOI progress; `report_fetch_state`
owns reusable provider success independently of item failures. `works` and
fetched provider facts remain metadata caches, not project graph authority.
Project `metric_requested_revision` is the request counter; projection
revision/watermark describe completed output. Legacy ingestion routes, importer,
DOI graph/event tables and the old metric function are removed without views.

