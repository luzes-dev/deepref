-- The contact address that integrations report is deployment configuration
-- (DEEPREF_CONTACT_EMAIL), not a workspace setting, so the column goes away.
-- Copy the value to the environment variable before applying this migration.
ALTER TABLE settings DROP COLUMN crossref_mailto;
