-- phase: expand
-- Onboarding state on users (handoff 03a). Columns only: nothing here scans or
-- rewrites `users` while the ALTER's lock is held.
--
-- `verified_email` is the address the provider vouched for at the last
-- sign-in (normalized, lowercase), or NULL when it did not. Authority that
-- depends on an address (invitations) requires it to equal the user's current
-- `primary_email`, so an email change by a binary that does not know this
-- column never inherits an earlier verification.
--
-- `onboarded_at` records that first-workspace onboarding (invitation or
-- personal workspace) completed. It is independent of the user insert: a user
-- an N-1 binary provisioned has no memberships and no `onboarded_at`, and the
-- next sign-in onboards them. Existing users who already hold a membership are
-- marked in bounded batches by `deepref-server backfill-project-workspaces`;
-- until then sign-in marks them without creating a workspace.
ALTER TABLE users ADD COLUMN verified_email text;
ALTER TABLE users ADD COLUMN onboarded_at timestamptz;
