-- phase: expand
-- Onboarding state on users (handoff 03a).
--
-- `email_verified` is the provider's verification of `primary_email`, refreshed
-- on every sign-in; it defaults to false so no row is trusted until the
-- provider says otherwise. Invitations bind to an address only while it is
-- verified.
--
-- `onboarded_at` records that first-workspace onboarding (invitation or
-- personal workspace) completed. It is independent of the user insert: a user
-- an N-1 binary provisioned has no memberships and no `onboarded_at`, and the
-- next sign-in onboards them. Users who already hold a membership at migration
-- time are marked onboarded; a user with none is left for sign-in to repair.
-- Both columns are nullable or constant-defaulted, so adding them rewrites
-- nothing.
ALTER TABLE users ADD COLUMN email_verified boolean NOT NULL DEFAULT false;
ALTER TABLE users ADD COLUMN onboarded_at timestamptz;

UPDATE users u
SET onboarded_at = now()
WHERE u.onboarded_at IS NULL
  AND EXISTS (SELECT 1 FROM workspace_memberships m WHERE m.user_id = u.id);
