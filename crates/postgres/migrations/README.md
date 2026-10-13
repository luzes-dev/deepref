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
  `deepref_postgres::test_isolation`.
